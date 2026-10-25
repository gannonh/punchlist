use axum::extract::FromRequestParts;
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method};
use percent_encoding::percent_decode_str;
use punchlist_core::Role;
use sha2::{Digest, Sha256};
use utoipa::Modify;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use uuid::Uuid;

use crate::{ApiError, AppState};

/// The actor a request's bearer token belongs to.
#[derive(Debug, Clone)]
pub struct Actor {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub role: Role,
    /// The agent key, such as `codex`. Only agents have one.
    pub kind: Option<String>,
    /// The hash of the token the request came with; none for an actor acting without one.
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

/// The cookie a browser signs in with (`POST /api/session`); its value is a person's token.
/// It is `SameSite=Strict`, so a browser sends it only on requests that Punchlist's own
/// pages make: that is the defence against cross-site request forgery, since no other
/// site's page can make a request that carries it.
pub const SESSION_COOKIE: &str = "punchlist_session";

/// The token in the request's session cookie.
fn session_token(headers: &HeaderMap) -> Option<&str> {
    (headers.get_all(COOKIE).iter())
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(SESSION_COOKIE)?.strip_prefix('='))
        .filter(|token| !token.is_empty())
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
    /// The row behind `/api/runs/{id}`, `/api/proof/{id}` or `/api/projects/{id}/milestones`,
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
        ["", "api", "projects", id, "milestones"] => match uuid(id) {
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
        let (token, from_cookie) = match parts.headers.get(AUTHORIZATION) {
            Some(value) => (
                (value.to_str().ok())
                    .and_then(|value| value.strip_prefix("Bearer "))
                    .ok_or(ApiError::Unauthorized)?,
                false,
            ),
            None => (
                session_token(&parts.headers).ok_or(ApiError::Unauthorized)?,
                true,
            ),
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
        let token_hash = hash_token(token);
        // A person's token is their account's: the actor is the account's person in the
        // workspace the request names, oldest workspace first.
        let memberships = sqlx::query!(
            r#"SELECT acc.id AS account_id, acc.name AS account_name, a.id, a.workspace_id,
                      a.name, w.issue_prefix
               FROM account acc
               JOIN actor a ON a.account_id = acc.id
               JOIN workspace w ON w.id = a.workspace_id
               WHERE acc.token_hash = $1
               ORDER BY w.created_at, w.id"#,
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
                // Who is signed in, and where they belong, needs no workspace.
                Named::Nothing
                    if memberships.len() == 1
                        || path == "/api/session"
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
                token_hash: Some(token_hash),
                account: Some(punchlist_api::Account {
                    id: actor.account_id,
                    name: actor.account_name.clone(),
                }),
            });
        }
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
        // Only a person signs in from a browser.
        if from_cookie && role != Role::Person {
            return Err(ApiError::Unauthorized);
        }
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
