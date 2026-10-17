//! Issues as responses carry them, and as the list, the board and the event stream show
//! them (`IssueRow`): one query for any set of a workspace's issues.

use chrono::{DateTime, Utc};
use punchlist_api::{
    ChecksState, GateSummary, Issue, IssueRow, MilestoneRef, ProjectRef, PullRequestState,
    PullRequestStatus, PullRequestSummary, RunSummary, plain_pull_request_url,
};
use punchlist_core::display_name;
use uuid::Uuid;

use crate::auth::parse_role;
use crate::issues::evaluate_gates;
use crate::workflow::active_workflow;

/// The issue, as every response that carries one shows it. `None` when it is not in the
/// workspace.
pub(crate) async fn issue(
    conn: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    id: &str,
) -> sqlx::Result<Option<Issue>> {
    let mut found = load(conn, workspace_id, Some(&[id.to_string()])).await?;
    Ok(found.pop().map(|(issue, _, _)| issue))
}

/// The rows of the workspace's issues in `ids`, or of all of them, oldest first. The gates
/// are those of each issue's next gated transition under the active workflow.
pub(crate) async fn issue_rows(
    conn: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    ids: Option<&[String]>,
) -> sqlx::Result<Vec<IssueRow>> {
    let loaded = load(conn, workspace_id, ids).await?;
    let (workflow, _) = active_workflow(conn, workspace_id).await?;
    let mut rows = Vec::with_capacity(loaded.len());
    for (issue, run, pull_request) in loaded {
        // ponytail: one gate evaluation, a few queries, per issue whose status has a gated
        // transition, so a first load of N such issues costs N evaluations. Upgrade path:
        // read the evidence of every issue in one set-based query and evaluate in memory.
        let gates = match workflow.next_gated(&issue.status) {
            Some(transition) => {
                let (_, results) =
                    evaluate_gates(conn, &workflow, transition, &issue.id, None).await?;
                let to = transition.to.as_str().to_string();
                Some(GateSummary {
                    to_name: display_name(&to),
                    to,
                    passed: results.iter().filter(|g| g.passed).count() as u32,
                    total: results.len() as u32,
                    first_failed: results.iter().find(|g| !g.passed).map(|g| g.gate.clone()),
                })
            }
            None => None,
        };
        rows.push(IssueRow {
            issue,
            run,
            pull_request,
            gates,
        });
    }
    Ok(rows)
}

struct Loaded {
    id: String,
    title: String,
    body: String,
    status: String,
    labels: Vec<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    parent_id: Option<String>,
    project_id: Option<Uuid>,
    project_name: Option<String>,
    milestone_id: Option<Uuid>,
    milestone_name: Option<String>,
    assignee_id: Option<Uuid>,
    assignee_name: Option<String>,
    assignee_role: Option<String>,
    assignee_kind: Option<String>,
    running_agent: Option<String>,
    running_since: Option<DateTime<Utc>>,
    queued_agent: Option<String>,
    last_agent: Option<String>,
    last_outcome: Option<String>,
    last_finished_at: Option<DateTime<Utc>>,
    last_reason: Option<String>,
    pr_number: Option<i64>,
    pr_state: Option<String>,
    pr_draft: Option<bool>,
    pr_checks: Option<String>,
    pr_url: Option<String>,
}

/// Each issue with its run and its newest pull request. The run is the running one; else
/// the dispatch waiting for a runner; else the latest run in the issue's status, when it
/// failed; else none.
async fn load(
    conn: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    ids: Option<&[String]>,
) -> sqlx::Result<Vec<(Issue, RunSummary, Option<PullRequestSummary>)>> {
    let rows = sqlx::query_as!(
        Loaded,
        r#"SELECT i.id, i.title, i.body, i.status, i.labels, i.created_at, i.updated_at,
                  i.parent_id, p.id AS "project_id?", p.name AS "project_name?",
                  m.id AS "milestone_id?", m.name AS "milestone_name?",
                  a.id AS "assignee_id?", a.name AS "assignee_name?",
                  a.role AS "assignee_role?", a.kind AS "assignee_kind?",
                  running.agent AS "running_agent?", running.started_at AS "running_since?",
                  waiting.agent AS "queued_agent?",
                  last.agent AS "last_agent?", last.outcome AS "last_outcome?",
                  last.finished_at AS "last_finished_at?", last.reason AS "last_reason?",
                  pr.number AS "pr_number?", pr.state AS "pr_state?", pr.draft AS "pr_draft?",
                  pr.checks AS "pr_checks?", pr.url AS "pr_url?"
           FROM issue i
           LEFT JOIN project p ON p.id = i.project_id
           LEFT JOIN milestone m ON m.id = i.milestone_id
           LEFT JOIN actor a ON a.id = i.assignee_id
           LEFT JOIN run running ON running.issue_id = i.id AND running.outcome = 'running'
           LEFT JOIN dispatch waiting ON waiting.issue_id = i.id AND waiting.run_id IS NULL
           LEFT JOIN LATERAL (
               SELECT r.agent, r.outcome, r.finished_at,
                      (SELECT t.failure_reason FROM attempt t WHERE t.run_id = r.id
                       ORDER BY t.attempt DESC LIMIT 1) AS reason
               FROM run r WHERE r.issue_id = i.id AND r.status = i.status
               ORDER BY r.started_at DESC LIMIT 1) last ON true
           LEFT JOIN LATERAL (
               SELECT p.number, p.state, p.draft, p.checks, p.url FROM pull_request p
               WHERE p.issue_id = i.id ORDER BY p.number DESC LIMIT 1) pr ON true
           WHERE i.workspace_id = $1 AND ($2::text[] IS NULL OR i.id = ANY($2))
           ORDER BY i.number"#,
        workspace_id,
        ids,
    )
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter().map(parse).collect()
}

fn parse(row: Loaded) -> sqlx::Result<(Issue, RunSummary, Option<PullRequestSummary>)> {
    let corrupt = |what: String| sqlx::Error::Decode(what.into());
    let assignee = match (row.assignee_id, row.assignee_name, row.assignee_role) {
        (Some(id), Some(name), Some(role)) => Some(punchlist_api::Actor {
            id,
            name,
            role: parse_role(&role)?,
            kind: row.assignee_kind,
        }),
        _ => None,
    };
    let run = match (row.running_agent, row.running_since, row.queued_agent) {
        (Some(agent), Some(since), _) => RunSummary::Running { agent, since },
        (_, _, Some(agent)) => RunSummary::Queued { agent },
        _ => match (
            row.last_agent,
            row.last_outcome.as_deref(),
            row.last_finished_at,
        ) {
            (Some(agent), Some("failed"), Some(at)) => RunSummary::Failed {
                agent,
                reason: row.last_reason,
                at,
            },
            _ => RunSummary::Idle,
        },
    };
    let pull_request = match (row.pr_number, row.pr_state, row.pr_draft, row.pr_checks) {
        (Some(number), Some(state), Some(draft), Some(checks)) => {
            let state = PullRequestState::parse(&state)
                .ok_or_else(|| corrupt(format!("unknown pull request state `{state}`")))?;
            let checks = ChecksState::parse(&checks)
                .ok_or_else(|| corrupt(format!("unknown checks state `{checks}`")))?;
            let url = (row.pr_url.as_deref())
                .and_then(|url| plain_pull_request_url(number, url))
                .map(str::to_string);
            Some(PullRequestSummary {
                number,
                state: PullRequestStatus::new(state, draft),
                checks,
                url,
            })
        }
        _ => None,
    };
    let issue = Issue {
        status_name: display_name(&row.status),
        id: row.id,
        title: row.title,
        body: row.body,
        status: row.status,
        assignee,
        labels: row.labels,
        parent: row.parent_id,
        project: (row.project_id.zip(row.project_name)).map(|(id, name)| ProjectRef { id, name }),
        milestone: (row.milestone_id.zip(row.milestone_name))
            .map(|(id, name)| MilestoneRef { id, name }),
        created_at: row.created_at,
        updated_at: row.updated_at,
        waiting_on_person: None,
    };
    Ok((issue, run, pull_request))
}
