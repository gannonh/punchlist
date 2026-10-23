//! `/api/actors`: a person mints agent actors and lists the workspace's actors.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use punchlist_api::{ActorInfo, ActorList, CreateAgent, CreatedAgent, ErrorBody};
use punchlist_core::{AGENT_KINDS, Role, agent_display_name};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::{Actor, hash_token, new_token, parse_role};
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(create_agent, list_actors))
}

struct ActorRow {
    id: Uuid,
    name: String,
    role: String,
    kind: Option<String>,
    model: Option<String>,
    owner: Option<String>,
    created_at: DateTime<Utc>,
}

impl TryFrom<ActorRow> for ActorInfo {
    type Error = sqlx::Error;

    fn try_from(row: ActorRow) -> Result<ActorInfo, sqlx::Error> {
        Ok(ActorInfo {
            id: row.id,
            name: row.name,
            role: parse_role(&row.role)?,
            kind: row.kind,
            model: row.model,
            owner: row.owner,
            created_at: row.created_at,
        })
    }
}

pub(crate) fn person_only(actor: &Actor) -> Result<(), ApiError> {
    if actor.role != Role::Person {
        return Err(ApiError::Forbidden("this needs a person's token".into()));
    }
    Ok(())
}

/// Mint an agent actor with its own token and no run. Its token reaches the workspace's
/// issues, creating and listing them and the event stream, projects, the workflow, actors and
/// proof files, and nothing else. Person token.
#[utoipa::path(
    post,
    path = "/api/actors",
    request_body = CreateAgent,
    responses(
        (status = 201, body = CreatedAgent),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "Not a person's token."),
        (status = 422, body = ErrorBody, description = "An unknown kind or owner, or a name over 100 characters."),
    )
)]
async fn create_agent(
    State(state): State<AppState>,
    actor: Actor,
    Json(request): Json<CreateAgent>,
) -> Result<(StatusCode, Json<CreatedAgent>), ApiError> {
    person_only(&actor)?;
    let kind = request.kind.trim();
    if !AGENT_KINDS.contains(&kind) {
        return Err(ApiError::Invalid(format!(
            "unknown agent kind `{kind}`; the kinds are {}",
            AGENT_KINDS.join(", ")
        )));
    }
    let name = match request.name.as_deref().map(str::trim) {
        None | Some("") => agent_display_name(kind),
        Some(name) if name.chars().count() > 100 => {
            return Err(ApiError::Invalid(
                "an agent's name is at most 100 characters".into(),
            ));
        }
        Some(name) => name.to_string(),
    };
    let model = request
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty());
    let owner_id = match request.owner.as_deref().map(str::trim) {
        None => actor.id,
        Some(owner) => {
            let people = sqlx::query_scalar!(
                "SELECT id FROM actor WHERE workspace_id = $1 AND role = 'person' AND name = $2",
                actor.workspace_id,
                owner,
            )
            .fetch_all(&state.pool)
            .await?;
            match people[..] {
                [id] => id,
                [] => {
                    return Err(ApiError::Invalid(format!(
                        "no person named `{owner}` in this workspace"
                    )));
                }
                _ => {
                    return Err(ApiError::Invalid(format!(
                        "{} people are named `{owner}` in this workspace",
                        people.len()
                    )));
                }
            }
        }
    };
    let token = new_token();
    let row = sqlx::query_as!(
        ActorRow,
        r#"WITH created AS (
               INSERT INTO actor (workspace_id, name, role, token_hash, kind, model, owner_id)
               VALUES ($1, $2, 'agent', $3, $4, $5, $6)
               RETURNING id, name, role, kind, model, owner_id, created_at
           )
           SELECT c.id, c.name, c.role, c.kind, c.model, o.name AS "owner?", c.created_at
           FROM created c JOIN actor o ON o.id = c.owner_id"#,
        actor.workspace_id,
        name,
        hash_token(&token),
        kind,
        model,
        owner_id,
    )
    .fetch_one(&state.pool)
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedAgent {
            actor: row.try_into()?,
            token,
        }),
    ))
}

/// List the workspace's people, agents and runners, oldest first. A person's or a
/// standalone agent's token.
#[utoipa::path(
    get,
    path = "/api/actors",
    responses(
        (status = 200, body = ActorList),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "A runner's or a run's agent's token."),
    )
)]
async fn list_actors(
    State(state): State<AppState>,
    actor: Actor,
) -> Result<Json<ActorList>, ApiError> {
    if actor.role == Role::Runner {
        return Err(ApiError::Forbidden(
            "this needs a person's or an agent's token".into(),
        ));
    }
    let rows = sqlx::query_as!(
        ActorRow,
        r#"SELECT a.id, a.name, a.role, a.kind, a.model, o.name AS "owner?", a.created_at
           FROM actor a LEFT JOIN actor o ON o.id = a.owner_id
           WHERE a.workspace_id = $1 AND a.role <> 'github'
           ORDER BY a.created_at, a.id"#,
        actor.workspace_id,
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(ActorList {
        actors: rows
            .into_iter()
            .map(ActorInfo::try_from)
            .collect::<Result<_, _>>()?,
    }))
}
