//! The workspace's workflow: the version loaded from `.punchlist/` on the repository's
//! default branch, or the built-in one until a valid version loads (ADR 0010). Also
//! `GET /api/workflow` and the JSON Schema for `workflow.toml`.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use punchlist_api::{ErrorBody, WorkflowInfo};
use punchlist_core::{Workflow, default_workflow, workflow_schema};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::Actor;
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(show_workflow))
        .routes(routes!(schema))
}

/// The workspace's active workflow and the commit it came from (`None` for the built-in
/// one).
pub(crate) async fn active_workflow(
    conn: &mut sqlx::PgConnection,
    workspace_id: Uuid,
) -> sqlx::Result<(Workflow, Option<String>)> {
    let row = sqlx::query!(
        "SELECT v.workflow_toml, v.prompts, v.commit_sha
         FROM workspace w JOIN workflow_version v ON v.id = w.workflow_version_id
         WHERE w.id = $1",
        workspace_id,
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Ok((default_workflow().clone(), None));
    };
    // Only versions that loaded are stored, so one that no longer does is corrupt data.
    let corrupt = |e: String| sqlx::Error::Decode(format!("stored workflow: {e}").into());
    let prompts: BTreeMap<String, String> =
        serde_json::from_value(row.prompts).map_err(|e| corrupt(e.to_string()))?;
    let workflow =
        Workflow::from_files(&row.workflow_toml, prompts).map_err(|e| corrupt(e.to_string()))?;
    Ok((workflow, Some(row.commit_sha)))
}

/// The payload of a `workflow_load` job.
#[derive(Debug, Serialize, Deserialize)]
struct LoadJob {
    workspace_id: Uuid,
}

/// Queues a load of the workspace's workflow from its repository's default branch. `key`
/// makes queueing the same load twice a no-op.
pub async fn queue_workflow_load(
    conn: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    key: &str,
) -> sqlx::Result<()> {
    let payload = serde_json::to_value(LoadJob { workspace_id })
        .map_err(|e| sqlx::Error::Encode(e.into()))?;
    sqlx::query!(
        "INSERT INTO job (kind, idempotency_key, payload) VALUES ('workflow_load', $1, $2)
         ON CONFLICT (idempotency_key) DO NOTHING",
        key,
        payload,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Reads `.punchlist/workflow.toml` and its prompts at the head of the default branch and
/// makes them the active version. A file that does not load is refused with its line and
/// reason in the log, and the active version stays. Runs inside the job's transaction.
pub(crate) async fn load(
    state: &AppState,
    tx: &mut sqlx::PgConnection,
    payload: serde_json::Value,
) -> anyhow::Result<()> {
    let LoadJob { workspace_id } = serde_json::from_value(payload)?;
    let github = state
        .github
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no GitHub App is configured"))?;
    let repository = sqlx::query!(
        "SELECT id, owner, name FROM repository WHERE workspace_id = $1",
        workspace_id
    )
    .fetch_one(&mut *tx)
    .await?;
    let files = github
        .default_branch(&repository.owner, &repository.name)
        .await?;
    let head = files.head.clone();
    let place = format!(
        "{}/{} {} at {}",
        repository.owner, repository.name, head.branch, head.sha
    );
    sqlx::query!(
        "UPDATE repository SET default_branch = $2 WHERE id = $1",
        repository.id,
        head.branch,
    )
    .execute(&mut *tx)
    .await?;
    let Some(text) = files.punchlist_file("workflow.toml").await? else {
        tracing::info!("no .punchlist/workflow.toml on {place}; the workflow stays as it is");
        return Ok(());
    };
    let refused = |error: punchlist_core::LoadError| {
        tracing::warn!("refused .punchlist/workflow.toml on {place}: {error}");
        Ok(())
    };
    let parsed = match Workflow::from_toml(&text) {
        Ok(parsed) => parsed,
        Err(error) => return refused(error),
    };
    let mut prompts = BTreeMap::new();
    for path in parsed.prompt_paths() {
        if let Some(prompt) = files.punchlist_file(&path).await? {
            prompts.insert(path, prompt);
        }
    }
    let workflow = match Workflow::from_files(&text, prompts) {
        Ok(workflow) => workflow,
        Err(error) => return refused(error),
    };
    let version_id = sqlx::query_scalar!(
        "INSERT INTO workflow_version (workspace_id, hash, commit_sha, workflow_toml, prompts)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (workspace_id, hash) DO UPDATE SET commit_sha = EXCLUDED.commit_sha
         RETURNING id",
        workspace_id,
        workflow.version(),
        head.sha,
        workflow.source(),
        serde_json::to_value(workflow.prompts())?,
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE workspace SET workflow_version_id = $2 WHERE id = $1",
        workspace_id,
        version_id,
    )
    .execute(&mut *tx)
    .await?;
    tracing::info!("loaded workflow {} from {place}", workflow.version());
    Ok(())
}

/// The workspace's active workflow: its version, the commit it came from, its statuses.
#[utoipa::path(
    get,
    path = "/api/workflow",
    responses((status = 200, body = WorkflowInfo), (status = 401, body = ErrorBody))
)]
async fn show_workflow(
    State(state): State<AppState>,
    actor: Actor,
) -> Result<Json<WorkflowInfo>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    let (workflow, commit_sha) = active_workflow(&mut conn, actor.workspace_id).await?;
    Ok(Json(WorkflowInfo {
        version: workflow.version().to_string(),
        commit_sha,
        statuses: workflow
            .statuses()
            .iter()
            .map(|s| s.as_str().to_string())
            .collect(),
    }))
}

/// The JSON Schema for `.punchlist/workflow.toml`. Unauthenticated, so editors can fetch it.
#[utoipa::path(
    get,
    path = "/api/workflow/schema.json",
    responses((status = 200, description = "A JSON Schema (draft 2020-12).", body = Object)),
    security(())
)]
async fn schema() -> Json<schemars::Schema> {
    Json(workflow_schema())
}
