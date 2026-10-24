//! `/api/runners`: register a runner, list runners and record heartbeats.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use punchlist_api::{
    ErrorBody, Heartbeat, HeartbeatReply, HeldAttempt, Lease, RegisterRunner, RegisteredRunner,
    Repository, Runner, RunnerList,
};
use punchlist_core::{Role, unknown_agent_kind};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::{Actor, hash_token, new_token};
use crate::workflow::lock_workspace;
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(register_runner, list_runners))
        .routes(routes!(heartbeat))
        .routes(routes!(leases))
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

/// The runner `id`, when it is the actor's own. Another workspace's runner, or none, is 404,
/// and another runner of this workspace is forbidden.
async fn own_runner(state: &AppState, actor: &Actor, id: Uuid) -> Result<RunnerRow, ApiError> {
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
    let own = runner_of(state, actor).await?;
    if own.id != id {
        return Err(ApiError::Forbidden(
            "a runner can only act as itself".into(),
        ));
    }
    Ok(own)
}

/// Register a runner. Needs a person's token. Creates the runner's own actor. A name this
/// person already registered in the workspace re-adopts the newest such runner instead: the
/// same id and actor, the request's agents, and a new token, so the old one stops working.
/// That is how a restarted runner keeps the attempts it holds (`GET /api/runners/{id}/leases`),
/// and why two runners started with one name cannot both run.
#[utoipa::path(
    post,
    path = "/api/runners",
    request_body = RegisterRunner,
    responses(
        (status = 201, body = RegisteredRunner, description = "Registered, or re-adopted with a new token."),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "Not a person's token."),
        (status = 422, body = ErrorBody),
    )
)]
pub(crate) async fn register_runner(
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
    if let Some(unknown) = agents.iter().find_map(|agent| unknown_agent_kind(agent)) {
        return Err(ApiError::Invalid(unknown));
    }
    let token = new_token();
    let mut tx = state.pool.begin().await?;
    // Two registrations of one name serialize here, so they cannot both create a runner.
    // Older data may hold one name twice, so the name has no unique index.
    lock_workspace(&mut tx, actor.workspace_id).await?;
    let adopted = sqlx::query_scalar!(
        "SELECT actor_id FROM runner WHERE workspace_id = $1 AND name = $2 AND registered_by = $3
         ORDER BY registered_at DESC, id DESC LIMIT 1",
        actor.workspace_id,
        name,
        actor.id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let row = if let Some(actor_id) = adopted {
        sqlx::query!(
            "UPDATE actor SET token_hash = $2 WHERE id = $1",
            actor_id,
            hash_token(&token),
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query_as!(
            RunnerRow,
            "UPDATE runner SET agents = $2, last_heartbeat = now() WHERE actor_id = $1
             RETURNING id, name, agents, registered_at, last_heartbeat",
            actor_id,
            &agents,
        )
        .fetch_one(&mut *tx)
        .await?
    } else {
        let actor_id = sqlx::query_scalar!(
            "INSERT INTO actor (workspace_id, name, role, token_hash)
             VALUES ($1, $2, 'runner', $3) RETURNING id",
            actor.workspace_id,
            name,
            hash_token(&token),
        )
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query_as!(
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
        .await?
    };
    sqlx::query!(
        "UPDATE runner SET models = $2 WHERE id = $1",
        row.id,
        serde_json::to_value(&request.models).map_err(|error| sqlx::Error::Encode(error.into()))?,
    )
    .execute(&mut *tx)
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
pub(crate) async fn list_runners(
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

/// Record a heartbeat and renew the leases of the attempts the body lists as held. A running
/// attempt that is not listed, or whose lease already lapsed, is not renewed: it expires and
/// the sweep fails it. The answer lists the held attempts that were not renewed (`stale`),
/// so the runner stops their agents: the issue of one whose lease lapsed is dispatched again.
/// The body is optional; without it no lease is renewed. Runner token, for the runner's own
/// id only.
#[utoipa::path(
    post,
    path = "/api/runners/{id}/heartbeat",
    params(("id" = Uuid, Path, description = "Runner id")),
    request_body(content = Heartbeat, description = "Optional."),
    responses(
        (status = 200, body = HeartbeatReply),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
async fn heartbeat(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    body: Bytes,
) -> Result<Json<HeartbeatReply>, ApiError> {
    let held: Heartbeat = if body.iter().all(u8::is_ascii_whitespace) {
        Heartbeat::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::Invalid(format!("invalid heartbeat body: {e}")))?
    };
    own_runner(&state, &actor, id).await?;
    let run_ids: Vec<Uuid> = held.held.iter().map(|h| h.run_id).collect();
    let attempts: Vec<i32> = held.held.iter().map(|h| h.attempt).collect();
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query_as!(
        RunnerRow,
        "UPDATE runner SET last_heartbeat = now() WHERE id = $1
         RETURNING id, name, agents, registered_at, last_heartbeat",
        id,
    )
    .fetch_one(&mut *tx)
    .await?;
    let renewed = sqlx::query_as!(
        HeldAttempt,
        "UPDATE attempt SET lease_expires_at = now() + make_interval(secs => $2)
         WHERE runner_id = $1 AND state = 'running' AND lease_expires_at > now()
             AND (run_id, attempt) IN (SELECT * FROM UNNEST($3::uuid[], $4::int[]))
         RETURNING run_id, attempt",
        id,
        state.lease.as_secs_f64(),
        &run_ids,
        &attempts,
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let stale = (held.held.into_iter())
        .filter(|held| !renewed.contains(held))
        .collect();
    Ok(Json(HeartbeatReply {
        runner: row.into(),
        stale,
    }))
}

/// The running attempts the runner holds whose leases have not lapsed, oldest first. A
/// runner that restarts reads these: it resumes each one it can (`POST /api/runs/{id}/resume`)
/// and reports the rest lost. Read-only. Runner token, for the runner's own id only.
#[utoipa::path(
    get,
    path = "/api/runners/{id}/leases",
    params(("id" = Uuid, Path, description = "Runner id")),
    responses(
        (status = 200, body = Vec<Lease>),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
async fn leases(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<Lease>>, ApiError> {
    own_runner(&state, &actor, id).await?;
    let rows = sqlx::query!(
        "SELECT a.run_id, a.attempt, r.agent, a.issue_id, r.branch, a.lease_expires_at,
                p.owner, p.name, p.default_branch
         FROM attempt a JOIN run r ON r.id = a.run_id
         JOIN repository p ON p.workspace_id = r.workspace_id
         WHERE a.runner_id = $1 AND a.state = 'running' AND a.lease_expires_at > now()
         ORDER BY a.started_at, a.id",
        id,
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| Lease {
                run_id: row.run_id,
                attempt: row.attempt,
                agent: row.agent,
                issue_id: row.issue_id,
                branch: row.branch,
                repository: Repository {
                    owner: row.owner,
                    name: row.name,
                    default_branch: row.default_branch,
                },
                lease_expires_at: row.lease_expires_at,
            })
            .collect(),
    ))
}
