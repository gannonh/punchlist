//! `/api/runners`: register a runner, list runners and record heartbeats.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use punchlist_api::{ErrorBody, RegisterRunner, RegisteredRunner, Runner, RunnerList};
use punchlist_core::Role;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::{Actor, hash_token, new_token};
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(register_runner, list_runners))
        .routes(routes!(heartbeat))
}

pub(crate) struct RunnerRow {
    pub id: Uuid,
    pub name: String,
    pub agents: Vec<String>,
    pub registered_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
}

impl From<RunnerRow> for Runner {
    fn from(row: RunnerRow) -> Runner {
        Runner {
            id: row.id,
            name: row.name,
            agents: row.agents,
            registered_at: row.registered_at,
            last_heartbeat: row.last_heartbeat,
        }
    }
}

/// The runner row behind a runner actor. Anyone else is forbidden.
pub(crate) async fn runner_of(state: &AppState, actor: &Actor) -> Result<RunnerRow, ApiError> {
    if actor.role != Role::Runner {
        return Err(ApiError::Forbidden("this needs a runner token".into()));
    }
    sqlx::query_as!(
        RunnerRow,
        "SELECT id, name, agents, registered_at, last_heartbeat FROM runner WHERE actor_id = $1",
        actor.id,
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::Forbidden("this actor has no runner".into()))
}

/// Register a runner. Needs a person's token. Creates the runner's own actor.
#[utoipa::path(
    post,
    path = "/api/runners",
    request_body = RegisterRunner,
    responses(
        (status = 201, body = RegisteredRunner),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "Not a person's token."),
        (status = 422, body = ErrorBody),
    )
)]
async fn register_runner(
    State(state): State<AppState>,
    actor: Actor,
    Json(request): Json<RegisterRunner>,
) -> Result<(StatusCode, Json<RegisteredRunner>), ApiError> {
    if actor.role != Role::Person {
        return Err(ApiError::Forbidden(
            "only a person can register a runner".into(),
        ));
    }
    let name = request.name.trim();
    if name.is_empty() {
        return Err(ApiError::Invalid("a runner needs a name".into()));
    }
    let agents: Vec<String> = request
        .agents
        .iter()
        .map(|agent| agent.trim().to_string())
        .collect();
    if agents.is_empty() || agents.iter().any(String::is_empty) {
        return Err(ApiError::Invalid(
            "a runner needs at least one agent key".into(),
        ));
    }
    let token = new_token();
    let mut tx = state.pool.begin().await?;
    let actor_id = sqlx::query_scalar!(
        "INSERT INTO actor (workspace_id, name, role, token_hash)
         VALUES ($1, $2, 'runner', $3) RETURNING id",
        actor.workspace_id,
        name,
        hash_token(&token),
    )
    .fetch_one(&mut *tx)
    .await?;
    let row = sqlx::query_as!(
        RunnerRow,
        "INSERT INTO runner (workspace_id, actor_id, name, agents, registered_by)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id, name, agents, registered_at, last_heartbeat",
        actor.workspace_id,
        actor_id,
        name,
        &agents,
        actor.id,
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(RegisteredRunner {
            runner: row.into(),
            token,
            lease_seconds: state.lease.as_secs_f64().ceil() as u32,
        }),
    ))
}

/// List the workspace's runners, oldest first.
#[utoipa::path(
    get,
    path = "/api/runners",
    responses((status = 200, body = RunnerList), (status = 401, body = ErrorBody))
)]
async fn list_runners(
    State(state): State<AppState>,
    actor: Actor,
) -> Result<Json<RunnerList>, ApiError> {
    let rows = sqlx::query_as!(
        RunnerRow,
        "SELECT id, name, agents, registered_at, last_heartbeat
         FROM runner WHERE workspace_id = $1 ORDER BY registered_at, id",
        actor.workspace_id,
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(RunnerList {
        runners: rows.into_iter().map(Runner::from).collect(),
    }))
}

/// Record a heartbeat and renew the leases on the runner's running attempts. Runner token,
/// for the runner's own id only.
#[utoipa::path(
    post,
    path = "/api/runners/{id}/heartbeat",
    params(("id" = Uuid, Path, description = "Runner id")),
    responses(
        (status = 200, body = Runner),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
async fn heartbeat(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> Result<Json<Runner>, ApiError> {
    let known = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM runner WHERE id = $1 AND workspace_id = $2) AS "exists!""#,
        id,
        actor.workspace_id,
    )
    .fetch_one(&state.pool)
    .await?;
    if !known {
        return Err(ApiError::RunnerNotFound(id));
    }
    let own = runner_of(&state, &actor).await?;
    if own.id != id {
        return Err(ApiError::Forbidden(
            "a runner can only send its own heartbeat".into(),
        ));
    }
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query_as!(
        RunnerRow,
        "UPDATE runner SET last_heartbeat = now() WHERE id = $1
         RETURNING id, name, agents, registered_at, last_heartbeat",
        id,
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE attempt SET lease_expires_at = now() + make_interval(secs => $2)
         WHERE runner_id = $1 AND state = 'running'",
        id,
        state.lease.as_secs_f64(),
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(row.into()))
}
