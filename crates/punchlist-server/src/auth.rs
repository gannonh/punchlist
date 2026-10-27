use axum::extract::FromRequestParts;
use axum::http::header::{AUTHORIZATION, COOKIE, HOST, ORIGIN};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method};
use percent_encoding::percent_decode_str;
use punchlist_core::Role;
use sha2::{Digest, Sha256};
use utoipa::Modify;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use uuid::Uuid;

use crate::{ApiError, AppState};

/// The actor a request's bearer token or session belongs to.
#[derive(Debug, Clone)]
pub struct Actor {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub role: Role,
    /// The agent key, such as `codex`. Only agents have one.
    pub kind: Option<String>,
    /// The hash of the token the request came with; none for a session and an actor acting
    /// without one.
    pub token_hash: Option<String>,
    /// The account a person's token authenticates; the actor is its person in the workspace
    /// the request names (ADR 0011). Agents and runners have none.
    pub account: Option<punchlist_api::Account>,
}

impl From<&Actor> for punchlist_api::Actor {
    fn from(actor: &Actor) -> punchlist_api::Actor {
        punchlist_api::Actor {
            id: actor.id,
            name: actor.name.clone(),
            role: actor.role,
            kind: actor.kind.clone(),
        }
    }
}

/// The cookie a browser signs in with (`POST /api/session`, or GitHub): its value is a random
/// session id, whose SHA-256 is a `session` row's id (ADR 0013). It is `SameSite=Lax`, so a
/// browser sends it on another site's top-level navigations and forms too; `same_origin`
/// refuses any write that carries it from another origin.
pub const SESSION_COOKIE: &str = "punchlist_session";

/// The named cookie's value in the request.
pub fn request_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    (headers.get_all(COOKIE).iter())
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(name)?.strip_prefix('='))
        .filter(|value| !value.is_empty())
}

/// The session id in the request's session cookie.
pub fn session_id(headers: &HeaderMap) -> Option<&str> {
    request_cookie(headers, SESSION_COOKIE)
}

/// The request's `Authorization: Bearer` value.
pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    (headers.get(AUTHORIZATION)?.to_str().ok())?.strip_prefix("Bearer ")
}

/// The account of a live session of `kind`, by the hash of its id: a browser's (`browser`)
/// comes as a cookie, `pl`'s (`cli`) as a bearer value, and neither the other way. A use
/// pushes its expiry out, 30 days for a browser's and 90 for `pl`'s, written at most once a
/// minute.
pub async fn session_account(
    pool: &sqlx::PgPool,
    session_hash: &str,
    kind: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"WITH found AS (
               SELECT id, account_id, last_used_at, kind FROM session
               WHERE id = $1 AND kind = $2 AND expires_at > now()
           ), touched AS (
               UPDATE session
               SET last_used_at = now(),
                   expires_at = now() + CASE found.kind WHEN 'cli' THEN interval '90 days'
                                                        ELSE interval '30 days' END
               FROM found
               WHERE session.id = found.id AND found.last_used_at < now() - interval '1 minute'
           )
           SELECT account_id FROM found"#,
        session_hash,
        kind,
    )
    .fetch_optional(pool)
    .await
}

/// Refuses a write (any method but `GET`, `HEAD` and `OPTIONS`) from another origin: one whose
/// `Origin` is not the instance's own, or one that carries the session cookie and no `Origin`.
/// A browser sends `Origin` with every such request; `pl`, runners and GitHub's deliveries
/// send neither it nor the cookie.
pub async fn same_origin(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let headers = request.headers();
    let safe = matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    let origin = headers.get(ORIGIN).and_then(|value| value.to_str().ok());
    if safe || (origin.is_none() && session_id(headers).is_none()) {
        return next.run(request).await;
    }
    if origin.is_some_and(|origin| own_origin(&state, headers, origin)) {
        return next.run(request).await;
    }
    tracing::warn!(
        origin,
        path = request.uri().path(),
        "refused a write from another origin"
    );
    ApiError::Forbidden("refused a write from another origin".into()).into_response()
}

/// Whether `origin` is the instance's: `PUNCHLIST_PUBLIC_URL`, else the request's own host,
/// which a page on another site cannot set.
fn own_origin(state: &AppState, headers: &HeaderMap, origin: &str) -> bool {
    match state.public_url.as_deref() {
        Some(url) => origin == url,
        None => origin.split_once("://").is_some_and(|(_, authority)| {
            [HOST.as_str(), "x-forwarded-host"]
                .iter()
                .any(|name| headers.get(*name).is_some_and(|host| host == authority))
        }),
    }
}

/// Reads an `actor.role` column. Its `CHECK` constraint allows only known roles, so an
/// unknown one is corrupt data, not a bad request.
pub fn parse_role(value: &str) -> Result<Role, sqlx::Error> {
    Role::parse(value)
        .ok_or_else(|| sqlx::Error::Decode(format!("unknown actor role `{value}`").into()))
}

/// A fresh bearer token. Only its hash is stored.
pub fn new_token() -> String {
    format!("plt_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// SHA-256 of a bearer token, in hex. Only the hash is stored.
pub fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The workspace a request names by its path (ADR 0011).
enum Named {
    /// No workspace: a person's account must be in exactly one.
    Nothing,
    /// `/api/workspaces/{id}/...`. A workspace the token is not in is 404; an id that is not
    /// a UUID is nil, which no token is in.
    Workspace(Uuid),
    /// `/api/issues/{id}/...`: the issue's workspace, none when there is no such issue (the
    /// handler's 404). Another workspace's issue is 403.
    Issue(Option<Uuid>),
    /// The row behind `/api/runs/{id}`, `/api/proof/{id}` or `/api/projects/{id}` and below,
    /// none when there is no such row. Its handler refuses a row of another workspace.
    Row(Option<Uuid>),
}

async fn named_workspace(
    pool: &sqlx::PgPool,
    path: &str,
    issue_id: Option<&str>,
) -> Result<Named, ApiError> {
    if let Some(issue_id) = issue_id {
        let workspace =
            sqlx::query_scalar!("SELECT workspace_id FROM issue WHERE id = $1", issue_id)
                .fetch_optional(pool)
                .await?;
        return Ok(Named::Issue(workspace));
    }
    let segments: Vec<&str> = path.split('/').collect();
    let uuid = |id: &str| Uuid::parse_str(id).ok();
    let row = match segments.as_slice() {
        ["", "api", "workspaces", id, ..] => {
            return Ok(Named::Workspace(uuid(id).unwrap_or_default()));
        }
        ["", "api", "runs", id, ..] if *id != "claim" => match uuid(id) {
            Some(id) => {
                sqlx::query_scalar!("SELECT workspace_id FROM run WHERE id = $1", id)
                    .fetch_optional(pool)
                    .await?
            }
            None => None,
        },
        ["", "api", "proof", id] => match uuid(id) {
            Some(id) => {
                sqlx::query_scalar!(
                    "SELECT i.workspace_id FROM proof p JOIN issue i ON i.id = p.issue_id
                     WHERE p.id = $1",
                    id
                )
                .fetch_optional(pool)
                .await?
            }
            None => None,
        },
        ["", "api", "projects", id, ..] => match uuid(id) {
            Some(id) => {
                sqlx::query_scalar!("SELECT workspace_id FROM project WHERE id = $1", id)
                    .fetch_optional(pool)
                    .await?
            }
            None => None,
        },
        _ => return Ok(Named::Nothing),
    };
    Ok(Named::Row(row))
}

/// `by` names the token's account, or its actor and the actor's workspace.
fn mismatch(by: String, issue_workspace: Uuid, issue: &str) -> ApiError {
    tracing::warn!(
        by,
        %issue_workspace,
        issue,
        "workspace mismatch: refused a request for another workspace's issue"
    );
    ApiError::Forbidden(format!("issue {issue} is in another workspace"))
}

impl FromRequestParts<AppState> for Actor {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Actor, ApiError> {
        // A bearer value is a CLI session of an account (`pl auth login`), else a person's
        // account's token, else an agent's or a runner's. A session cookie is a browser
        // session of an account.
        let (token_hash, session) = match parts.headers.get(AUTHORIZATION) {
            Some(_) => {
                let hash = hash_token(bearer(&parts.headers).ok_or(ApiError::Unauthorized)?);
                match session_account(&state.pool, &hash, "cli").await? {
                    Some(account) => (None, Some(account)),
                    None => (Some(hash), None),
                }
            }
            None => {
                let id = session_id(&parts.headers).ok_or(ApiError::Unauthorized)?;
                let account = session_account(&state.pool, &hash_token(id), "browser")
                    .await?
                    .ok_or(ApiError::Unauthorized)?;
                (None, Some(account))
            }
        };
        let path = parts.uri.path();
        // The issue a path names: `/api/issues/<id>` and below. Decoded as the route's `Path`
        // decodes it, so `PL%2D1` is checked as the `PL-1` the handler acts on.
        let issue_id = path
            .strip_prefix("/api/issues/")
            .and_then(|rest| rest.split('/').next())
            .and_then(|id| percent_decode_str(id).decode_utf8().ok())
            .filter(|id| !id.is_empty());
        let issue_id = issue_id.as_deref();
        let named = named_workspace(&state.pool, path, issue_id).await?;
        // The actor of an account is its person in the workspace the request names, oldest
        // workspace first.
        let memberships = sqlx::query!(
            r#"SELECT acc.id AS account_id, acc.name AS account_name, acc.github_login, a.id,
                      a.workspace_id, a.name, w.issue_prefix
               FROM account acc
               JOIN actor a ON a.account_id = acc.id
               JOIN workspace w ON w.id = a.workspace_id
               WHERE acc.id = $1 OR acc.token_hash = $2
               ORDER BY w.created_at, w.id"#,
            session,
            token_hash,
        )
        .fetch_all(&state.pool)
        .await?;
        if let Some(oldest) = memberships.first() {
            let member = |workspace| memberships.iter().find(|m| m.workspace_id == workspace);
            let actor = match named {
                Named::Workspace(id) => member(id).ok_or(ApiError::WorkspaceNotFound(id))?,
                Named::Issue(Some(id)) => member(id).ok_or_else(|| {
                    let by = format!("account {}", oldest.account_id);
                    mismatch(by, id, issue_id.unwrap_or_default())
                })?,
                Named::Row(Some(id)) => member(id).unwrap_or(oldest),
                Named::Issue(None) | Named::Row(None) => oldest,
                // Who is signed in, where they belong and their sessions need no workspace.
                Named::Nothing
                    if memberships.len() == 1
                        || path == "/api/session"
                        || path == "/api/sessions"
                        || path.starts_with("/api/sessions/")
                        || path.starts_with("/api/cli-logins/")
                        || path == "/api/workspaces" =>
                {
                    oldest
                }
                Named::Nothing => {
                    let mut prefixes: Vec<&str> = memberships
                        .iter()
                        .map(|m| m.issue_prefix.as_str())
                        .collect();
                    prefixes.sort_unstable();
                    return Err(ApiError::WorkspaceNotNamed(prefixes.join(", ")));
                }
            };
            return Ok(Actor {
                id: actor.id,
                workspace_id: actor.workspace_id,
                name: actor.name.clone(),
                role: Role::Person,
                kind: None,
                token_hash,
                account: Some(punchlist_api::Account {
                    id: actor.account_id,
                    name: actor.account_name.clone(),
                    github_login: actor.github_login.clone(),
                }),
            });
        }
        // A session is a person's, in at least one workspace.
        let token_hash = token_hash.ok_or(ApiError::Unauthorized)?;
        let row = sqlx::query!(
            r#"SELECT a.id, a.workspace_id, a.name, a.role, a.kind, a.run_id,
                      r.outcome AS "outcome?"
               FROM actor a LEFT JOIN run r ON r.id = a.run_id
               WHERE a.token_hash = $1"#,
            token_hash,
        )
        .fetch_optional(&state.pool)
        .await?
        .ok_or(ApiError::Unauthorized)?;
        let role = parse_role(&row.role)?;
        // A run's agent says its own run waits on a person, which that route refuses once the
        // run has ended.
        let own_waiting = parts.method == Method::POST
            && (row.run_id).is_some_and(|run| path == format!("/api/runs/{run}/waiting"));
        // A run's agent's token works only while its run is running, but for that route.
        if role == Role::Agent
            && row.run_id.is_some()
            && row.outcome.as_deref() != Some("running")
            && !own_waiting
        {
            return Err(ApiError::Unauthorized);
        }
        // No token reaches another workspace. An issue that does not exist anywhere is the
        // handler's 404.
        match named {
            Named::Workspace(id) if id != row.workspace_id => {
                return Err(ApiError::WorkspaceNotFound(id));
            }
            Named::Issue(Some(id)) if id != row.workspace_id => {
                let by = format!("actor {} of workspace {}", row.id, row.workspace_id);
                return Err(mismatch(by, id, issue_id.unwrap_or_default()));
            }
            _ => {}
        }
        // An agent, while its run runs if it has one, reaches any issue of its workspace,
        // `/api/issues/<id>` and below, and the few reads and the one write it needs to file,
        // list and watch them (`agent_route`); a run's agent also reads run logs, to review
        // another run's work. The workflow's `by` rules and locks still judge each move.
        // Everything else, such as sessions, runners, minting actors and creating projects, is
        // for people and runners.
        if role == Role::Agent
            && !own_waiting
            && issue_id.is_none()
            && !agent_route(&parts.method, path, row.run_id.is_some())
        {
            return Err(ApiError::Forbidden(
                "an agent acts only on the workspace's issues".into(),
            ));
        }
        Ok(Actor {
            id: row.id,
            workspace_id: row.workspace_id,
            role,
            name: row.name,
            kind: row.kind,
            token_hash: Some(token_hash),
            account: None,
        })
    }
}

/// The routes outside `/api/issues/<id>` an agent's token reaches, and a run's agent's the
/// run logs too. Each handler keeps it to the token's workspace.
fn agent_route(method: &Method, path: &str, run_agent: bool) -> bool {
    let route = (crate::workspaces::workspace_route(path))
        .or_else(|| path.strip_prefix("/api"))
        .unwrap_or(path);
    let segments: Vec<&str> = route.split('/').collect();
    match (method.as_str(), segments.as_slice()) {
        ("GET", ["", "runs", _, "log"]) => run_agent,
        ("GET" | "POST", ["", "issues"])
        | ("GET", ["", "workspaces", _, "events"])
        | ("GET", ["", "projects"])
        | ("GET", ["", "workflow"])
        | ("GET", ["", "actors"])
        | ("GET", ["", "proof", _])
        | ("GET", ["", "session"])
        | ("GET", ["", "workspaces"]) => true,
        _ => false,
    }
}

pub struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
    }
}
