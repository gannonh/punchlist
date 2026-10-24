//! The workspace's workflow: the version loaded from `.punchlist/` on the repository's
//! default branch, or the built-in one until a valid version loads (ADR 0010). Also
//! `GET /api/workflow` and the JSON Schema for `workflow.toml`.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use punchlist_api::{ErrorBody, StatusInfo, WorkflowInfo, WorkflowRefusal};
use punchlist_core::{Status, Workflow, default_workflow, workflow_schema};
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
    let (workflow, commit_sha, _) = stored_workflow(conn, workspace_id).await?;
    Ok((workflow, commit_sha))
}

/// `active_workflow`, and why a push of that version would be refused today, if it would.
/// Such a version stays in force as stored (PRD, "The workflow file").
pub(crate) async fn stored_workflow(
    conn: &mut sqlx::PgConnection,
    workspace_id: Uuid,
) -> sqlx::Result<(Workflow, Option<String>, Option<String>)> {
    let row = sqlx::query!(
        "SELECT v.workflow_toml, v.prompts, v.commit_sha
         FROM workspace w JOIN workflow_version v ON v.id = w.workflow_version_id
         WHERE w.id = $1",
        workspace_id,
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Ok((default_workflow().clone(), None, None));
    };
    let (workflow, invalid) = parse_stored(&row.workflow_toml, row.prompts)?;
    Ok((workflow, Some(row.commit_sha), invalid))
}

/// A `workflow_version` row as a workflow, and why a push of it would be refused today.
fn parse_stored(
    workflow_toml: &str,
    prompts: serde_json::Value,
) -> sqlx::Result<(Workflow, Option<String>)> {
    // Only versions that loaded are stored, so one that no longer parses is corrupt data. A
    // version stored before a runtime rule (KAT-3750) comes back with that rule's problem.
    let corrupt = |e: String| sqlx::Error::Decode(format!("stored workflow: {e}").into());
    let prompts: BTreeMap<String, String> =
        serde_json::from_value(prompts).map_err(|e| corrupt(e.to_string()))?;
    let (workflow, invalid) =
        Workflow::from_stored(workflow_toml, prompts).map_err(|e| corrupt(e.to_string()))?;
    Ok((workflow, invalid.map(|e| e.to_string())))
}

/// The workflow version the issue's waiting dispatch was queued under: the one its
/// transition recorded. The dispatch's agent is that version's, so the run's prompt is too,
/// whatever version has loaded since.
pub(crate) async fn dispatch_workflow(
    conn: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    issue_id: &str,
) -> sqlx::Result<Workflow> {
    let row = sqlx::query!(
        "SELECT v.workflow_toml, v.prompts
         FROM dispatch d JOIN transition t ON t.id = d.transition_id
         JOIN workflow_version v ON v.workspace_id = $1 AND v.hash = t.workflow_version
         WHERE d.issue_id = $2 AND d.run_id IS NULL",
        workspace_id,
        issue_id,
    )
    .fetch_optional(&mut *conn)
    .await?;
    match row {
        Some(row) => Ok(parse_stored(&row.workflow_toml, row.prompts)?.0),
        // No stored version has that hash: the transition was made under the built-in one.
        None => Ok(default_workflow().clone()),
    }
}

/// Logs each workspace whose active version a push would be refused for today, and returns
/// how many there are. Called once when the server starts: a version is valid when it
/// loads, so only a newer server can find one.
pub async fn warn_invalid_workflows(pool: &sqlx::PgPool) -> sqlx::Result<usize> {
    let mut conn = pool.acquire().await?;
    let mut invalid = 0;
    for workspace_id in sqlx::query_scalar!("SELECT id FROM workspace ORDER BY created_at")
        .fetch_all(&mut *conn)
        .await?
    {
        match stored_workflow(&mut conn, workspace_id).await {
            Ok((_, _, None)) => {}
            Ok((workflow, _, Some(problem))) => {
                invalid += 1;
                tracing::warn!(
                    "workspace {workspace_id}: the active workflow {} fails validation: \
                     {problem}; it stays in force until a valid .punchlist/workflow.toml is pushed",
                    workflow.version()
                );
            }
            Err(error) => tracing::error!("workspace {workspace_id}: {error}"),
        }
    }
    Ok(invalid)
}

/// Locks the workspace row until the transaction ends. Every transaction that changes an
/// issue's status or creates an issue takes it first, before any issue row lock, so `load`
/// cannot switch workflows while a move decided against the old one is in flight, and
/// two paths never take the two locks in opposite orders.
pub(crate) async fn lock_workspace(
    conn: &mut sqlx::PgConnection,
    workspace_id: Uuid,
) -> sqlx::Result<()> {
    sqlx::query!(
        "SELECT id FROM workspace WHERE id = $1 FOR NO KEY UPDATE",
        workspace_id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(())
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

/// Queues a load of every workspace's workflow, once per `start`. GitHub does not redeliver
/// a push that reached a stopped server, so the server reads the default branch when it
/// starts; a file with the active version's hash changes nothing.
pub async fn queue_workflow_loads_at_start(pool: &sqlx::PgPool, start: Uuid) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO job (kind, idempotency_key, payload)
         SELECT 'workflow_load', 'workflow_load:start:' || $1::uuid || ':' || id,
                jsonb_build_object('workspace_id', id)
         FROM workspace ORDER BY created_at
         ON CONFLICT (idempotency_key) DO NOTHING",
        start,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Why a workflow that drops statuses issues are in is refused.
fn dropped_message<'a>(in_use: impl Iterator<Item = (&'a str, i64)>) -> String {
    let in_use: Vec<_> = in_use
        .map(|(status, issues)| format!("`{status}` by {issues} issue(s)"))
        .collect();
    format!(
        "status in use is not in the workflow: {}",
        in_use.join(", ")
    )
}

/// Reads `.punchlist/workflow.toml` and its prompts at the head of the default branch and
/// makes them the active version. A file that does not load is refused with its line and
/// reason in the log and on the workspace, and the active version stays. Runs inside the
/// job's transaction.
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
    // The refusal is committed with the job, so `GET /api/workflow` can say why the push
    // did not load.
    let refused = async |tx: &mut sqlx::PgConnection, reason: String| {
        tracing::warn!("refused .punchlist/workflow.toml on {place}: {reason}");
        sqlx::query!(
            "UPDATE workspace SET workflow_refused_commit = $2, workflow_refused_at = now(),
                 workflow_refused_reason = $3 WHERE id = $1",
            workspace_id,
            head.sha,
            reason,
        )
        .execute(tx)
        .await?;
        Ok(())
    };
    let parsed = match Workflow::from_toml(&text) {
        Ok(parsed) => parsed,
        Err(error) => return refused(&mut *tx, error.to_string()).await,
    };
    let mut prompts = BTreeMap::new();
    for path in parsed.prompt_paths() {
        if let Some(prompt) = files.punchlist_file(&path).await? {
            prompts.insert(path, prompt);
        }
    }
    let workflow = match Workflow::from_files(&text, prompts) {
        Ok(workflow) => workflow,
        Err(error) => return refused(&mut *tx, error.to_string()).await,
    };
    // Waits for moves in flight and holds off new ones until commit (`lock_workspace`), so
    // no issue can enter a dropped status between this check and the switch.
    lock_workspace(&mut *tx, workspace_id).await?;
    let dropped = sqlx::query!(
        r#"SELECT status, count(*) AS "issues!" FROM issue
           WHERE workspace_id = $1 AND status <> ALL($2) GROUP BY status ORDER BY status"#,
        workspace_id,
        &workflow
            .statuses()
            .iter()
            .map(|s| s.as_str().to_string())
            .collect::<Vec<_>>(),
    )
    .fetch_all(&mut *tx)
    .await?;
    if !dropped.is_empty() {
        let reason = dropped_message(dropped.iter().map(|d| (d.status.as_str(), d.issues)));
        return refused(&mut *tx, reason).await;
    }
    // The active version keeps the commit it was loaded from; an earlier version loaded again
    // takes the head's.
    let version_id = sqlx::query_scalar!(
        "INSERT INTO workflow_version (workspace_id, hash, commit_sha, workflow_toml, prompts)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (workspace_id, hash) DO UPDATE SET commit_sha = CASE
             WHEN workflow_version.id IN (SELECT workflow_version_id FROM workspace WHERE id = $1)
             THEN workflow_version.commit_sha ELSE EXCLUDED.commit_sha END
         RETURNING id",
        workspace_id,
        workflow.version(),
        head.sha,
        workflow.source(),
        serde_json::to_value(workflow.prompts())?,
    )
    .fetch_one(&mut *tx)
    .await?;
    let previous = sqlx::query_scalar!(
        "SELECT workflow_version_id FROM workspace WHERE id = $1",
        workspace_id,
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE workspace SET workflow_version_id = $2, workflow_refused_commit = NULL,
             workflow_refused_at = NULL, workflow_refused_reason = NULL WHERE id = $1",
        workspace_id,
        version_id,
    )
    .execute(&mut *tx)
    .await?;
    // A different version can change every row's gates and status names, so the open app
    // is told each row changed.
    // ponytail: one event per issue per workflow change, written under the workspace lock, so
    // a load costs O(issues) statements and event rows. Upgrade path: one workspace-level stream
    // message on which the app refetches the list and the workflow once.
    if previous != Some(version_id) {
        let issues = sqlx::query_scalar!(
            "SELECT id FROM issue WHERE workspace_id = $1 ORDER BY id",
            workspace_id,
        )
        .fetch_all(&mut *tx)
        .await?;
        // `github_actor` creates the actor, so a workspace without issues gets none.
        if !issues.is_empty() {
            let github = crate::github::github_actor(&mut *tx, workspace_id, None).await?;
            for issue_id in &issues {
                crate::events::record_row_event(
                    &mut *tx,
                    workspace_id,
                    issue_id,
                    github,
                    "workflow",
                )
                .await?;
            }
        }
    }
    tracing::info!("loaded workflow {} from {place}", workflow.version());
    Ok(())
}

/// A status as the app reads it: its name, the roles it locks out, its dispatch, and whether
/// any transition leaves it.
pub(crate) fn status_info(workflow: &Workflow, status: &Status) -> StatusInfo {
    StatusInfo {
        key: status.as_str().to_string(),
        name: status.display_name(),
        locked_to: (workflow.locks().iter())
            .find(|(locked, _)| locked == status)
            .map(|(_, roles)| roles.clone())
            .unwrap_or_default(),
        dispatch: workflow
            .dispatch_for(status.as_str())
            .map(|d| d.agent.clone()),
        closed: !(workflow.transitions().iter()).any(|transition| transition.from.contains(status)),
    }
}

/// The workspace's active workflow: its version, the commit it came from, its statuses and
/// the last push that was refused.
#[utoipa::path(
    get,
    path = "/api/workflow",
    responses((status = 200, body = WorkflowInfo), (status = 401, body = ErrorBody))
)]
pub(crate) async fn show_workflow(
    State(state): State<AppState>,
    actor: Actor,
) -> Result<Json<WorkflowInfo>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    let (workflow, commit_sha, invalid) = stored_workflow(&mut conn, actor.workspace_id).await?;
    let refused = sqlx::query!(
        "SELECT workflow_refused_commit, workflow_refused_at, workflow_refused_reason
         FROM workspace WHERE id = $1",
        actor.workspace_id,
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(Json(WorkflowInfo {
        version: workflow.version().to_string(),
        invalid,
        commit_sha,
        statuses: (workflow.statuses().iter())
            .map(|status| status_info(&workflow, status))
            .collect(),
        refused: refused
            .workflow_refused_commit
            .zip(refused.workflow_refused_at)
            .zip(refused.workflow_refused_reason)
            .map(|((commit_sha, at), reason)| WorkflowRefusal {
                commit_sha,
                at,
                reason,
            }),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusal_names_each_status_and_its_issue_count() {
        assert_eq!(
            dropped_message([("done", 2), ("todo", 1)].into_iter()),
            "status in use is not in the workflow: `done` by 2 issue(s), `todo` by 1 issue(s)"
        );
    }
}
