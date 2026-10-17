//! The event log (ADR 0006): sequence numbers, reading events back, and
//! `GET /api/workspaces/{id}/events`, the stream the web app keeps its cache current with.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{self, KeepAlive, Sse};
use chrono::{DateTime, Utc};
use futures_util::Stream;
use punchlist_api::{ErrorBody, Event, EventDetail, StreamItem};
use punchlist_core::display_name;
use serde::Deserialize;
use sqlx::PgPool;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::{Actor, parse_role};
use crate::rows::issue_rows;
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(event_stream))
}

/// Takes the workspace's next event sequence number, for an event the caller writes in the
/// same transaction.
///
/// The `UPDATE` holds the workspace row until the transaction ends, so the next writer
/// waits here until this one commits or rolls back. Events therefore commit in sequence
/// order: once a reader sees an event, every event with a lower number is visible too, and
/// reading `seq > after` in order skips none.
pub(crate) async fn next_event_seq(
    conn: &mut sqlx::PgConnection,
    workspace_id: Uuid,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        "UPDATE workspace SET last_event_seq = last_event_seq + 1 WHERE id = $1
         RETURNING last_event_seq",
        workspace_id,
    )
    .fetch_one(&mut *conn)
    .await
}

struct EventRow {
    seq: i64,
    issue_id: String,
    created_at: DateTime<Utc>,
    actor_id: Uuid,
    actor_name: String,
    actor_role: String,
    actor_kind: Option<String>,
    kind: String,
    from_status: Option<String>,
    to_status: Option<String>,
    workflow_version: Option<String>,
    delivery_id: Option<String>,
    comment_body: Option<String>,
    gate: Option<String>,
    reason: Option<String>,
    edited_fields: Option<Vec<String>>,
}

impl TryFrom<EventRow> for Event {
    type Error = sqlx::Error;

    fn try_from(row: EventRow) -> Result<Event, sqlx::Error> {
        let corrupt = |what: &str| sqlx::Error::Decode(format!("{what} event row").into());
        let detail = match (row.kind.as_str(), row.comment_body) {
            ("comment", Some(body)) => EventDetail::Comment { body },
            ("created", None) => EventDetail::Created,
            ("edited", None) => EventDetail::Edited {
                fields: row
                    .edited_fields
                    .ok_or_else(|| corrupt("incomplete edited"))?,
            },
            ("refusal", None) => match (
                row.from_status,
                row.to_status,
                row.gate,
                row.reason,
                row.workflow_version,
            ) {
                (Some(from), Some(to), Some(gate), Some(reason), Some(version)) => {
                    EventDetail::Refusal {
                        from_name: display_name(&from),
                        to_name: display_name(&to),
                        from,
                        to,
                        gate,
                        reason,
                        workflow_version: version,
                        delivery_id: row.delivery_id,
                    }
                }
                _ => return Err(corrupt("incomplete refusal")),
            },
            ("transition", None) => match (row.from_status, row.to_status, row.workflow_version) {
                (Some(from), Some(to), Some(version)) => EventDetail::Transition {
                    from_name: display_name(&from),
                    to_name: display_name(&to),
                    from,
                    to,
                    workflow_version: version,
                    delivery_id: row.delivery_id,
                },
                _ => return Err(corrupt("incomplete transition")),
            },
            _ => return Err(corrupt("unknown")),
        };
        Ok(Event {
            seq: row.seq,
            issue_id: row.issue_id,
            created_at: row.created_at,
            actor: punchlist_api::Actor {
                id: row.actor_id,
                name: row.actor_name,
                role: parse_role(&row.actor_role)?,
                kind: row.actor_kind,
            },
            detail,
        })
    }
}

/// The workspace's events after `after`, of one issue or of all, in sequence order, at most
/// `limit` of them.
pub(crate) async fn events(
    conn: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    issue_id: Option<&str>,
    after: i64,
    limit: i64,
) -> sqlx::Result<Vec<Event>> {
    let rows = sqlx::query_as!(
        EventRow,
        r#"SELECT e.seq, e.issue_id, e.created_at,
                a.id AS actor_id, a.name AS actor_name, a.role AS actor_role,
                a.kind AS actor_kind, e.kind,
                coalesce(t.from_status, r.from_status) AS "from_status?",
                coalesce(t.to_status, r.to_status) AS "to_status?",
                coalesce(t.workflow_version, r.workflow_version) AS "workflow_version?",
                coalesce(t.delivery_id, r.delivery_id) AS "delivery_id?",
                c.body AS "comment_body?", r.gate AS "gate?", r.reason AS "reason?",
                e.edited_fields
         FROM event e
         JOIN actor a ON a.id = e.actor_id
         LEFT JOIN transition t ON t.id = e.transition_id
         LEFT JOIN comment c ON c.id = e.comment_id
         LEFT JOIN refusal r ON r.id = e.refusal_id
         WHERE e.workspace_id = $1 AND ($2::text IS NULL OR e.issue_id = $2) AND e.seq > $3
         ORDER BY e.seq
         LIMIT $4"#,
        workspace_id,
        issue_id,
        after,
        limit,
    )
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter().map(Event::try_from).collect()
}

#[derive(Deserialize, IntoParams)]
struct After {
    /// Stream the events after this sequence number: the list's `last_event_seq`. A
    /// reconnecting `EventSource`'s `Last-Event-ID` takes its place.
    #[serde(default)]
    after: i64,
}

/// How often an open stream looks for new events.
const POLL: Duration = Duration::from_millis(500);
/// The most events one read takes.
const BATCH: i64 = 100;

/// The workspace's events after `after`, as Server-Sent Events: each one's `id` is its
/// sequence number and its data a `StreamItem`, with the issue's row as it is when the
/// event is sent. The stream stays open.
#[utoipa::path(
    get,
    path = "/api/workspaces/{id}/events",
    params(("id" = Uuid, Path, description = "The token's workspace"), After),
    responses(
        (status = 200, content_type = "text/event-stream", body = StreamItem,
         description = "One `issue` message per event, in sequence order."),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody, description = "Not the token's workspace."),
    )
)]
async fn event_stream(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Query(query): Query<After>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<sse::Event, Infallible>>>, ApiError> {
    if id != actor.workspace_id {
        return Err(ApiError::WorkspaceNotFound(id));
    }
    let after = (headers.get("last-event-id"))
        .and_then(|value| value.to_str().ok()?.parse().ok())
        .unwrap_or(query.after);
    let stream = futures_util::stream::unfold(
        (state.pool, id, after, VecDeque::new()),
        |(pool, workspace_id, mut after, mut pending)| async move {
            loop {
                if let Some(message) = pending.pop_front() {
                    return Some((Ok(message), (pool, workspace_id, after, pending)));
                }
                match next_items(&pool, workspace_id, after).await {
                    Ok(items) if items.is_empty() => {}
                    Ok(items) => {
                        for item in items {
                            after = item.event.seq;
                            pending.push_back(message(&item));
                        }
                        continue;
                    }
                    Err(error) => tracing::error!(%error, "read the event stream"),
                }
                // ponytail: every open stream polls with one indexed query per 500 ms. Upgrade
                // path: a NOTIFY trigger on `event` and one PgListener waking the streams.
                tokio::time::sleep(POLL).await;
            }
        },
    );
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// The next events after `after` with their issues' rows, read in one snapshot.
async fn next_items(
    pool: &PgPool,
    workspace_id: Uuid,
    after: i64,
) -> sqlx::Result<Vec<StreamItem>> {
    let mut tx = pool.begin().await?;
    sqlx::query!("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let events = events(&mut tx, workspace_id, None, after, BATCH).await?;
    if events.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids: Vec<String> = events.iter().map(|e| e.issue_id.clone()).collect();
    ids.sort();
    ids.dedup();
    let rows = issue_rows(&mut tx, workspace_id, Some(&ids)).await?;
    tx.commit().await?;
    (events.into_iter())
        .map(|event| {
            let row = (rows.iter().find(|r| r.issue.id == event.issue_id).cloned())
                .ok_or_else(|| sqlx::Error::Decode("an event's issue is missing".into()))?;
            Ok(StreamItem { event, row })
        })
        .collect()
}

fn message(item: &StreamItem) -> sse::Event {
    sse::Event::default()
        .id(item.event.seq.to_string())
        .event("issue")
        .json_data(item)
        .expect("a stream item serializes")
}
