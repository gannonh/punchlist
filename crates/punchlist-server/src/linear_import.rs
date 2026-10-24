//! Importing a Linear project (`pl import linear`). `pl` reads Linear, so the API key never
//! reaches the server, and posts what it read to `POST /api/workspaces/{id}/imports/linear`,
//! which queues a `linear_import` job; the job writes it. Every row it writes keeps its Linear
//! id, so a rerun finds it and writes nothing twice.

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use punchlist_api::{
    ErrorBody, ImportCounts, ImportReport, ImportRun, ImportState, ImportStatus, LinearImport,
    LinearIssue, LinearUser,
};
use punchlist_core::{Role, Status, Workflow, display_name};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::actors::person_only;
use crate::auth::{Actor, parse_role};
use crate::events::next_event_seq;
use crate::issues::{
    end_dispatches, insert_transition, is_signed_by_agent, record_event, record_transition_event,
};
use crate::jobs::Permanent;
use crate::workflow::{active_workflow, lock_workspace};
use crate::{ApiError, AppState};

/// A few hundred issues with their comments is a few megabytes, past axum's 2 MB default.
const MAX_BODY: usize = 64 * 1024 * 1024;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(start_import))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .routes(routes!(import_status))
        .routes(routes!(import_run))
}

/// The payload of a `linear_import` job.
#[derive(Debug, Serialize, Deserialize)]
struct ImportJob {
    workspace_id: Uuid,
    /// The person who asked for the import.
    actor_id: Uuid,
    snapshot: LinearImport,
}

/// The token's own workspace, for a person: another workspace is not found, as for the event
/// stream.
fn check_workspace(actor: &Actor, id: Uuid) -> Result<(), ApiError> {
    if id != actor.workspace_id {
        return Err(ApiError::WorkspaceNotFound(id));
    }
    person_only(actor)
}

/// Each issue's status under the workflow, in the snapshot's order, or why some have none:
/// each Linear state that maps to no status, named once.
fn map_states<'a>(
    workflow: &'a Workflow,
    issues: &[LinearIssue],
) -> Result<Vec<&'a Status>, String> {
    let mut unmapped: Vec<&str> = Vec::new();
    let mut statuses = Vec::with_capacity(issues.len());
    for issue in issues {
        match workflow.linear_status(&issue.state.name, &issue.state.kind) {
            Some(status) => statuses.push(status),
            None if unmapped.contains(&issue.state.name.as_str()) => {}
            None => unmapped.push(&issue.state.name),
        }
    }
    match unmapped.as_slice() {
        [] => Ok(statuses),
        [one] => Err(format!(
            "Linear state {one:?} maps to no status; map it under [import.linear.states] in .punchlist/workflow.toml"
        )),
        many => Err(format!(
            "Linear states {} map to no status; map them under [import.linear.states] in .punchlist/workflow.toml",
            many.iter()
                .map(|n| format!("{n:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// An issue id, or a comment id on one issue, that the snapshot repeats. The job keys each
/// row by Linear id, so a repeat would fail its insert on every retry.
fn repeated_id(issues: &[LinearIssue]) -> Option<String> {
    let mut seen = HashSet::new();
    for issue in issues {
        if !seen.insert(issue.id.as_str()) {
            return Some(format!("Linear issue {} appears twice", issue.identifier));
        }
        let mut comments = HashSet::new();
        if let Some(c) = issue
            .comments
            .iter()
            .find(|c| !comments.insert(c.id.as_str()))
        {
            return Some(format!(
                "Linear comment {} on {} appears twice",
                c.id, issue.identifier
            ));
        }
    }
    None
}

/// Import a Linear project, as `pl import linear` read it. Every issue's state must map to a
/// status of the active workflow, or nothing is queued. Person token.
#[utoipa::path(
    post,
    path = "/api/workspaces/{id}/imports/linear",
    params(("id" = Uuid, Path, description = "The token's workspace")),
    request_body = LinearImport,
    responses(
        (status = 202, body = ImportRun, description = "Queued."),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "Not a person's token."),
        (status = 404, body = ErrorBody, description = "Not the token's workspace."),
        (status = 422, body = ErrorBody, description = "A Linear state maps to no status, the message naming each, or an issue or comment appears twice."),
    )
)]
async fn start_import(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(snapshot): Json<LinearImport>,
) -> Result<(StatusCode, Json<ImportRun>), ApiError> {
    check_workspace(&actor, id)?;
    let mut tx = state.pool.begin().await?;
    let (workflow, _) = active_workflow(&mut tx, id).await?;
    map_states(&workflow, &snapshot.issues).map_err(ApiError::Invalid)?;
    if let Some(repeat) = repeated_id(&snapshot.issues) {
        return Err(ApiError::Invalid(repeat));
    }
    let payload = serde_json::to_value(ImportJob {
        workspace_id: id,
        actor_id: actor.id,
        snapshot,
    })
    .map_err(|e| sqlx::Error::Encode(e.into()))?;
    let row = sqlx::query!(
        "INSERT INTO job (kind, idempotency_key, payload) VALUES ('linear_import', $1, $2)
         RETURNING id, created_at",
        format!("linear_import:{}", Uuid::new_v4()),
        payload,
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    state.job_wake.notify_one();
    Ok((
        StatusCode::ACCEPTED,
        Json(ImportRun {
            id: row.id,
            state: ImportState::Queued,
            error: None,
            report: None,
            created_at: row.created_at,
            finished_at: None,
        }),
    ))
}

struct RunRow {
    id: i64,
    state: String,
    last_error: Option<String>,
    result: Option<serde_json::Value>,
    created_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
}

impl TryFrom<RunRow> for ImportRun {
    type Error = sqlx::Error;

    fn try_from(row: RunRow) -> Result<ImportRun, sqlx::Error> {
        let corrupt = |what: String| sqlx::Error::Decode(what.into());
        let state = ImportState::parse(&row.state)
            .ok_or_else(|| corrupt(format!("unknown job state `{}`", row.state)))?;
        let report = (row.result)
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| corrupt(format!("import report: {e}")))?;
        Ok(ImportRun {
            id: row.id,
            state,
            error: row.last_error,
            report,
            created_at: row.created_at,
            finished_at: row.finished_at,
        })
    }
}

/// One import of the workspace. Person token.
#[utoipa::path(
    get,
    path = "/api/workspaces/{id}/imports/{run}",
    params(
        ("id" = Uuid, Path, description = "The token's workspace"),
        ("run" = i64, Path, description = "The import's id"),
    ),
    responses(
        (status = 200, body = ImportRun),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "Not a person's token."),
        (status = 404, body = ErrorBody, description = "Not the token's workspace, or no such import in it."),
    )
)]
async fn import_run(
    State(state): State<AppState>,
    actor: Actor,
    Path((id, run)): Path<(Uuid, i64)>,
) -> Result<Json<ImportRun>, ApiError> {
    check_workspace(&actor, id)?;
    let row = sqlx::query_as!(
        RunRow,
        "SELECT id, state, last_error, result, created_at, finished_at FROM job
         WHERE id = $1 AND kind = 'linear_import' AND (payload->>'workspace_id')::uuid = $2",
        run,
        id,
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::ImportNotFound(run))?;
    Ok(Json(row.try_into()?))
}

/// What the workspace holds from Linear, counted from the rows with a Linear id, and its
/// latest import. Person token.
#[utoipa::path(
    get,
    path = "/api/workspaces/{id}/imports",
    params(("id" = Uuid, Path, description = "The token's workspace")),
    responses(
        (status = 200, body = ImportStatus),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "Not a person's token."),
        (status = 404, body = ErrorBody, description = "Not the token's workspace."),
    )
)]
async fn import_status(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> Result<Json<ImportStatus>, ApiError> {
    check_workspace(&actor, id)?;
    let totals = sqlx::query!(
        r#"SELECT
             (SELECT count(*) FROM issue WHERE workspace_id = $1 AND linear_id IS NOT NULL)
                 AS "issues!",
             (SELECT count(*) FROM comment c JOIN issue i ON i.id = c.issue_id
              WHERE i.workspace_id = $1 AND c.linear_id IS NOT NULL) AS "comments!",
             (SELECT count(*) FROM project WHERE workspace_id = $1 AND linear_id IS NOT NULL)
                 AS "projects!",
             (SELECT count(*) FROM milestone m JOIN project p ON p.id = m.project_id
              WHERE p.workspace_id = $1 AND m.linear_id IS NOT NULL) AS "milestones!",
             (SELECT count(*) FROM actor WHERE workspace_id = $1 AND linear_id IS NOT NULL)
                 AS "people!""#,
        id,
    )
    .fetch_one(&state.pool)
    .await?;
    let last_run = sqlx::query_as!(
        RunRow,
        "SELECT id, state, last_error, result, created_at, finished_at FROM job
         WHERE kind = 'linear_import' AND (payload->>'workspace_id')::uuid = $1
         ORDER BY id DESC LIMIT 1",
        id,
    )
    .fetch_optional(&state.pool)
    .await?
    .map(ImportRun::try_from)
    .transpose()?;
    Ok(Json(ImportStatus {
        totals: ImportCounts {
            issues: totals.issues,
            comments: totals.comments,
            projects: totals.projects,
            milestones: totals.milestones,
            people: totals.people,
        },
        last_run,
    }))
}

/// A row matched by its Linear id, else by its name with no Linear id yet.
struct Named {
    id: Uuid,
    linear_id: Option<String>,
    pass_when: String,
}

/// The one of `rows` that is the Linear record `linear_id`: the row with that id, else a row of
/// the same name not imported yet, which the import adopts. `Err` when only a row imported from
/// another Linear record has the name.
fn pick<'a>(rows: &'a [Named], linear_id: &str) -> Result<Option<&'a Named>, ()> {
    let mine = (rows
        .iter()
        .find(|r| r.linear_id.as_deref() == Some(linear_id)))
    .or_else(|| rows.iter().find(|r| r.linear_id.is_none()));
    match mine {
        None if !rows.is_empty() => Err(()),
        mine => Ok(mine),
    }
}

struct Existing {
    id: String,
    linear_id: String,
    linear_identifier: Option<String>,
    linear_updated_at: Option<DateTime<Utc>>,
    title: String,
    body: String,
    labels: Vec<String>,
    status: String,
    /// A `transition` event after the one that last set its status from Linear.
    moved: bool,
    parent_id: Option<String>,
    project_id: Option<Uuid>,
    milestone_id: Option<Uuid>,
}

/// An actor the import writes as. Only the id and workspace are read by the event writers.
fn acting(id: Uuid, workspace_id: Uuid, name: String, role: Role) -> Actor {
    Actor {
        id,
        workspace_id,
        name,
        role,
        kind: None,
        token_hash: None,
        account: None,
    }
}

/// The people and the imported agent the import writes as, created on first sight.
struct Actors {
    workspace_id: Uuid,
    importer: Actor,
    people: HashMap<String, Actor>,
    agent: Option<Actor>,
}

impl Actors {
    /// The person for a Linear user: the one imported before, else a new one with no token.
    async fn person(
        &mut self,
        tx: &mut sqlx::PgConnection,
        user: &LinearUser,
        created: &mut ImportCounts,
    ) -> sqlx::Result<Actor> {
        if let Some(actor) = self.people.get(&user.id) {
            return Ok(actor.clone());
        }
        let found = sqlx::query!(
            "SELECT id, name FROM actor WHERE workspace_id = $1 AND linear_id = $2",
            self.workspace_id,
            user.id,
        )
        .fetch_optional(&mut *tx)
        .await?;
        let (id, name) = match found {
            Some(row) => (row.id, row.name),
            None => {
                let name = match user.name.trim() {
                    "" => "Linear user",
                    name => name,
                };
                let id = sqlx::query_scalar!(
                    "INSERT INTO actor (workspace_id, name, role, linear_id)
                     VALUES ($1, $2, 'person', $3) RETURNING id",
                    self.workspace_id,
                    name,
                    user.id,
                )
                .fetch_one(&mut *tx)
                .await?;
                created.people += 1;
                (id, name.to_string())
            }
        };
        let actor = acting(id, self.workspace_id, name, Role::Person);
        self.people.insert(user.id.clone(), actor.clone());
        Ok(actor)
    }

    /// The workspace's imported agent, owned by the person whose import first needed it.
    async fn agent(&mut self, tx: &mut sqlx::PgConnection) -> sqlx::Result<Actor> {
        if let Some(agent) = &self.agent {
            return Ok(agent.clone());
        }
        let found = sqlx::query_scalar!(
            "SELECT id FROM actor WHERE workspace_id = $1 AND kind = 'imported'",
            self.workspace_id,
        )
        .fetch_optional(&mut *tx)
        .await?;
        let id = match found {
            Some(id) => id,
            None => {
                sqlx::query_scalar!(
                    "INSERT INTO actor (workspace_id, name, role, kind, owner_id)
                     VALUES ($1, 'Imported agent', 'agent', 'imported', $2) RETURNING id",
                    self.workspace_id,
                    self.importer.id,
                )
                .fetch_one(&mut *tx)
                .await?
            }
        };
        let agent = acting(id, self.workspace_id, "Imported agent".into(), Role::Agent);
        self.agent = Some(agent.clone());
        Ok(agent)
    }
}

/// Labels trimmed, without empty ones or repeats, in Linear's order.
fn labels_of(issue: &LinearIssue) -> Vec<String> {
    let mut labels: Vec<String> = Vec::new();
    for label in issue.labels.iter().map(|l| l.trim()) {
        if !label.is_empty() && !labels.iter().any(|l| l == label) {
            labels.push(label.to_string());
        }
    }
    labels
}

fn title_of(issue: &LinearIssue) -> &str {
    match issue.title.trim() {
        "" => &issue.identifier,
        title => title,
    }
}

/// Runs a `linear_import` job in the job's transaction and returns its report. A Linear state
/// that maps to no status under the workflow active now fails the job for good.
// ponytail: the whole import is one transaction under the workspace lock, inside the job's
// 30 s lease; a few hundred issues take a few seconds. Upgrade path: a batch per transaction
// with the lease renewed between batches.
pub(crate) async fn run(
    tx: &mut sqlx::PgConnection,
    payload: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let ImportJob {
        workspace_id,
        actor_id,
        snapshot,
    } = serde_json::from_value(payload)?;
    lock_workspace(tx, workspace_id).await?;
    let (workflow, _) = active_workflow(tx, workspace_id).await?;
    let statuses = map_states(&workflow, &snapshot.issues).map_err(Permanent)?;
    let importer = sqlx::query!(
        "SELECT name, role FROM actor WHERE id = $1 AND workspace_id = $2",
        actor_id,
        workspace_id,
    )
    .fetch_one(&mut *tx)
    .await?;
    let importer = acting(
        actor_id,
        workspace_id,
        importer.name,
        parse_role(&importer.role)?,
    );
    let mut report = ImportReport::default();
    let mut actors = Actors {
        workspace_id,
        importer: importer.clone(),
        people: HashMap::new(),
        agent: None,
    };

    let project = &snapshot.project;
    let rows = sqlx::query_as!(
        Named,
        r#"SELECT id, linear_id, '' AS "pass_when!" FROM project
           WHERE workspace_id = $1 AND (linear_id = $2 OR name = $3)"#,
        workspace_id,
        project.id,
        project.name.trim(),
    )
    .fetch_all(&mut *tx)
    .await?;
    let project_id = match pick(&rows, &project.id) {
        Err(()) => {
            return Err(Permanent(format!(
                "project {} is already imported from another Linear project",
                project.name.trim()
            ))
            .into());
        }
        Ok(Some(row)) => {
            if row.linear_id.is_none() {
                sqlx::query!(
                    "UPDATE project SET linear_id = $2 WHERE id = $1",
                    row.id,
                    project.id
                )
                .execute(&mut *tx)
                .await?;
                report.updated.projects += 1;
            }
            row.id
        }
        Ok(None) => {
            report.created.projects += 1;
            sqlx::query_scalar!(
                "INSERT INTO project (workspace_id, name, linear_id) VALUES ($1, $2, $3)
                 RETURNING id",
                workspace_id,
                project.name.trim(),
                project.id,
            )
            .fetch_one(&mut *tx)
            .await?
        }
    };

    let mut milestones: HashMap<&str, Uuid> = HashMap::new();
    for milestone in &project.milestones {
        let pass_when = milestone.description.as_deref().unwrap_or("").trim();
        let rows = sqlx::query_as!(
            Named,
            "SELECT id, linear_id, pass_when FROM milestone
             WHERE project_id = $1 AND (linear_id = $2 OR name = $3)",
            project_id,
            milestone.id,
            milestone.name.trim(),
        )
        .fetch_all(&mut *tx)
        .await?;
        let id = match pick(&rows, &milestone.id) {
            Err(()) => {
                return Err(Permanent(format!(
                    "milestone {} in project {} is already imported from another Linear milestone",
                    milestone.name.trim(),
                    project.name.trim()
                ))
                .into());
            }
            Ok(Some(row)) => {
                if row.linear_id.is_none() || row.pass_when != pass_when {
                    sqlx::query!(
                        "UPDATE milestone SET linear_id = $2, pass_when = $3 WHERE id = $1",
                        row.id,
                        milestone.id,
                        pass_when,
                    )
                    .execute(&mut *tx)
                    .await?;
                    report.updated.milestones += 1;
                }
                row.id
            }
            Ok(None) => {
                report.created.milestones += 1;
                sqlx::query_scalar!(
                    "INSERT INTO milestone (project_id, name, pass_when, linear_id)
                     VALUES ($1, $2, $3, $4) RETURNING id",
                    project_id,
                    milestone.name.trim(),
                    pass_when,
                    milestone.id,
                )
                .fetch_one(&mut *tx)
                .await?
            }
        };
        milestones.insert(&milestone.id, id);
    }

    let existing = sqlx::query_as!(
        Existing,
        r#"SELECT id, linear_id AS "linear_id!", linear_identifier, linear_updated_at, title, body,
                  labels, status, parent_id, project_id, milestone_id,
                  EXISTS (SELECT 1 FROM event e WHERE e.issue_id = issue.id
                          AND e.kind = 'transition' AND e.seq > coalesce(linear_status_seq, 0))
                    AS "moved!"
           FROM issue WHERE workspace_id = $1 AND linear_id IS NOT NULL ORDER BY number"#,
        workspace_id,
    )
    .fetch_all(&mut *tx)
    .await?;
    let by_linear: HashMap<&str, &Existing> =
        existing.iter().map(|e| (e.linear_id.as_str(), e)).collect();
    // Each Linear issue's Punchlist id, for every issue the workspace has from Linear.
    let mut ids: HashMap<String, String> = existing
        .iter()
        .map(|e| (e.linear_id.clone(), e.id.clone()))
        .collect();
    let issues = &snapshot.issues;
    let mut order: Vec<usize> = (0..issues.len()).collect();
    order.sort_by(|&a, &b| {
        (issues[a].created_at, &issues[a].identifier)
            .cmp(&(issues[b].created_at, &issues[b].identifier))
    });

    let mut created = Vec::new();
    let mut changed = Vec::new();
    for &i in &order {
        let issue = &issues[i];
        let milestone = issue
            .milestone_id
            .as_deref()
            .and_then(|m| milestones.get(m))
            .copied();
        match by_linear.get(issue.id.as_str()) {
            None => {
                let creator = match &issue.creator {
                    Some(user) => actors.person(tx, user, &mut report.created).await?,
                    None => importer.clone(),
                };
                let next = sqlx::query!(
                    r#"UPDATE workspace SET next_issue_number = next_issue_number + 1 WHERE id = $1
                       RETURNING issue_prefix, next_issue_number - 1 AS "number!""#,
                    workspace_id,
                )
                .fetch_one(&mut *tx)
                .await?;
                let id = format!("{}-{}", next.issue_prefix, next.number);
                sqlx::query!(
                    "INSERT INTO issue (id, workspace_id, number, title, body, status, created_by,
                                        created_at, labels, project_id, milestone_id, linear_id,
                                        linear_identifier, linear_url, linear_updated_at)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
                    id,
                    workspace_id,
                    next.number,
                    title_of(issue),
                    issue.description.as_deref().unwrap_or(""),
                    statuses[i].as_str(),
                    creator.id,
                    issue.created_at,
                    &labels_of(issue),
                    project_id,
                    milestone,
                    issue.id,
                    issue.identifier,
                    issue.url,
                    issue.updated_at,
                )
                .execute(&mut *tx)
                .await?;
                let seq = record_event(tx, &creator, &id, None, Some(issue.created_at)).await?;
                set_linear_status_seq(tx, &id, seq).await?;
                report.created.issues += 1;
                ids.insert(issue.id.clone(), id);
                created.push(i);
            }
            Some(e) if e.linear_updated_at.is_none_or(|at| issue.updated_at > at) => {
                changed.push((i, *e, milestone));
            }
            Some(_) => {}
        }
    }

    // Parents once every issue exists, so a parent created after its child is found.
    let parent_of = |i: usize, report: &mut ImportReport| -> Option<String> {
        let issue = &issues[i];
        let parent = issue.parent_id.as_deref()?;
        let found = ids.get(parent).cloned();
        if found.is_none() {
            report.notes.push(format!(
                "{} ({}) has a parent outside Linear's project, so it has none here",
                issue.identifier, ids[&issue.id]
            ));
        }
        found
    };
    for &i in &created {
        if let Some(parent) = parent_of(i, &mut report) {
            sqlx::query!(
                "UPDATE issue SET parent_id = $2 WHERE id = $1",
                ids[&issues[i].id],
                parent,
            )
            .execute(&mut *tx)
            .await?;
        }
    }
    for (i, current, milestone) in changed {
        let issue = &issues[i];
        let title = title_of(issue);
        let body = issue.description.as_deref().unwrap_or("");
        let labels = labels_of(issue);
        let parent = parent_of(i, &mut report);
        let fields: Vec<&str> = [
            ("title", title != current.title),
            ("body", body != current.body),
            ("labels", labels != current.labels),
            ("parent", parent != current.parent_id),
            ("project", Some(project_id) != current.project_id),
            ("milestone", milestone != current.milestone_id),
        ]
        .into_iter()
        .filter_map(|(field, changed)| changed.then_some(field))
        .collect();
        let mut to = statuses[i].as_str();
        // A rerun carries Linear's edits, but does not undo a move Punchlist recorded.
        if current.moved && to != current.status {
            report.notes.push(format!(
                "{} ({}) moved in Punchlist since its import, so it stays in {}; Linear has it in {}",
                issue.identifier,
                current.id,
                display_name(&current.status),
                issue.state.name
            ));
            to = &current.status;
        }
        sqlx::query!(
            "UPDATE issue SET title = $2, body = $3, labels = $4, parent_id = $5, project_id = $6,
                    milestone_id = $7, status = $8, linear_identifier = $9, linear_url = $10,
                    linear_updated_at = $11,
                    updated_at = CASE WHEN $12 THEN now() ELSE updated_at END
             WHERE id = $1",
            current.id,
            title,
            body,
            &labels,
            parent,
            project_id,
            milestone,
            to,
            issue.identifier,
            issue.url,
            issue.updated_at,
            !fields.is_empty() || to != current.status,
        )
        .execute(&mut *tx)
        .await?;
        if !fields.is_empty() {
            record_event(tx, &importer, &current.id, Some(&fields), None).await?;
        }
        if to != current.status {
            // As any move does, without queuing a dispatch for the new status.
            end_dispatches(tx, &importer, &current.id).await?;
            let transition = insert_transition(
                tx,
                &importer,
                &current.id,
                &current.status,
                to,
                None,
                workflow.version(),
            )
            .await?;
            let (seq, _) = record_transition_event(tx, &importer, &current.id, transition).await?;
            set_linear_status_seq(tx, &current.id, seq).await?;
        }
        if !fields.is_empty() || to != current.status {
            report.updated.issues += 1;
        }
    }

    // Comments in Linear's order across issues, each the first time it is seen.
    let have: HashSet<String> = sqlx::query_scalar!(
        r#"SELECT c.linear_id AS "linear_id!" FROM comment c JOIN issue i ON i.id = c.issue_id
           WHERE i.workspace_id = $1 AND c.linear_id IS NOT NULL"#,
        workspace_id,
    )
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .collect();
    let mut comments: Vec<_> = (issues.iter())
        .flat_map(|issue| issue.comments.iter().map(move |c| (issue, c)))
        .filter(|(_, c)| !have.contains(&c.id))
        .collect();
    comments.sort_by(|(_, a), (_, b)| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    for (issue, comment) in comments {
        let id = &ids[&issue.id];
        let body = comment.body.trim_end();
        if body.trim().is_empty() {
            report.notes.push(format!(
                "{} ({id}): skipped an empty comment from {}",
                issue.identifier,
                comment.created_at.format("%Y-%m-%d %H:%M UTC")
            ));
            continue;
        }
        let author = match &comment.user {
            Some(user) if !is_signed_by_agent(body) => {
                actors.person(tx, user, &mut report.created).await?
            }
            _ => actors.agent(tx).await?,
        };
        let comment_id = sqlx::query_scalar!(
            "INSERT INTO comment (issue_id, actor_id, body, created_at, linear_id)
             VALUES ($1, $2, $3, $4, $5) RETURNING id",
            id,
            author.id,
            body,
            comment.created_at,
            comment.id,
        )
        .fetch_one(&mut *tx)
        .await?;
        let seq = next_event_seq(tx, workspace_id).await?;
        sqlx::query!(
            "INSERT INTO event (workspace_id, seq, issue_id, kind, actor_id, comment_id, created_at)
             VALUES ($1, $2, $3, 'comment', $4, $5, $6)",
            workspace_id,
            seq,
            id,
            author.id,
            comment_id,
            comment.created_at,
        )
        .execute(&mut *tx)
        .await?;
        report.created.comments += 1;
    }

    let in_snapshot: HashSet<&str> = issues.iter().map(|i| i.id.as_str()).collect();
    for gone in existing
        .iter()
        .filter(|e| e.project_id == Some(project_id) && !in_snapshot.contains(e.linear_id.as_str()))
    {
        report.notes.push(format!(
            "{} ({}) is no longer in Linear's project",
            gone.linear_identifier.as_deref().unwrap_or(&gone.linear_id),
            gone.id
        ));
    }
    Ok(serde_json::to_value(report)?)
}

async fn set_linear_status_seq(
    tx: &mut sqlx::PgConnection,
    id: &str,
    seq: i64,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE issue SET linear_status_seq = $2 WHERE id = $1",
        id,
        seq
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use punchlist_api::LinearState;
    use punchlist_core::default_workflow;

    fn issue(state: &str, kind: &str) -> LinearIssue {
        LinearIssue {
            id: "a".into(),
            identifier: "KAT-1".into(),
            url: "https://linear.app/kata-sh/issue/KAT-1".into(),
            title: "T".into(),
            description: None,
            state: LinearState {
                name: state.into(),
                kind: kind.into(),
            },
            labels: Vec::new(),
            parent_id: None,
            milestone_id: None,
            creator: None,
            created_at: "2026-10-10T12:00:00Z".parse().unwrap(),
            updated_at: "2026-10-10T12:00:00Z".parse().unwrap(),
            comments: Vec::new(),
        }
    }

    #[test]
    fn unmapped_states_are_named_once_each() {
        let workflow = default_workflow();
        let mapped = map_states(
            workflow,
            &[issue("Rework", "started"), issue("Done", "completed")],
        )
        .unwrap();
        assert_eq!(
            mapped.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            ["in_progress", "done"]
        );
        assert_eq!(
            map_states(
                workflow,
                &[issue("Triage", "triage"), issue("Triage", "triage")]
            )
            .unwrap_err(),
            "Linear state \"Triage\" maps to no status; map it under [import.linear.states] in .punchlist/workflow.toml"
        );
        assert_eq!(
            map_states(
                workflow,
                &[issue("Triage", "triage"), issue("Inbox", "triage")]
            )
            .unwrap_err(),
            "Linear states \"Triage\", \"Inbox\" map to no status; map them under [import.linear.states] in .punchlist/workflow.toml"
        );
    }

    #[test]
    fn a_comment_is_an_agents_when_its_last_line_is_the_agent_line() {
        assert!(is_signed_by_agent("Opened #12.\n\n(agent)\n\n"));
        assert!(is_signed_by_agent("Done.\n  (agent)  "));
        assert!(!is_signed_by_agent("(agent)\nthen a person wrote more"));
        assert!(!is_signed_by_agent("Not (agent) here"));
        assert!(!is_signed_by_agent(""));
    }
}
