//! `/api/issues/{id}/proof` and `/api/proof/{id}`: files attached to an issue to show its
//! work, kept on the server's disk.

use std::path::Path as FsPath;

use axum::Json;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::StatusCode;
use axum::http::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use punchlist_api::{AttachProof, ErrorBody, PROOF_TYPES, Proof, ProofKind, proof_kind};
use tokio::io::AsyncWriteExt;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::{Actor, parse_role};
use crate::events::record_row_event;
use crate::issues::current_attempt;
use crate::workflow::{active_workflow, lock_workspace};
use crate::{ApiError, AppState};

/// Room in an upload's body for the multipart framing and the other fields, past the file.
const BODY_SLACK: usize = 64 * 1024;

pub fn routes(max_bytes: u64) -> OpenApiRouter<AppState> {
    // The file's own limit is checked as it streams; this one only has to be past it, so
    // axum's 2 MB default does not cut the body first.
    let limit = usize::try_from(max_bytes)
        .unwrap_or(usize::MAX)
        .saturating_add(BODY_SLACK);
    OpenApiRouter::new()
        .routes(routes!(attach_proof, list_proof))
        .layer(DefaultBodyLimit::max(limit))
        .routes(routes!(get_proof))
}

struct ProofRow {
    id: Uuid,
    issue_id: String,
    attempt: i32,
    kind: String,
    title: String,
    content_type: String,
    size: i64,
    actor_id: Uuid,
    actor_name: String,
    actor_role: String,
    actor_kind: Option<String>,
    created_at: DateTime<Utc>,
}

impl TryFrom<ProofRow> for Proof {
    type Error = sqlx::Error;

    fn try_from(row: ProofRow) -> Result<Proof, sqlx::Error> {
        // The column's CHECK constraint allows only known kinds.
        let kind = ProofKind::parse(&row.kind).ok_or_else(|| {
            sqlx::Error::Decode(format!("unknown proof kind `{}`", row.kind).into())
        })?;
        Ok(Proof {
            id: row.id,
            issue_id: row.issue_id,
            attempt: row.attempt,
            kind,
            title: row.title,
            content_type: row.content_type,
            size: row.size,
            actor: punchlist_api::Actor {
                id: row.actor_id,
                name: row.actor_name,
                role: parse_role(&row.actor_role)?,
                kind: row.actor_kind,
            },
            created_at: row.created_at,
        })
    }
}

/// The accepted content types, for a refusal.
fn accepted() -> String {
    let mut types: Vec<&str> = PROOF_TYPES.iter().map(|(_, ty, _)| *ty).collect();
    types.dedup();
    types.join(", ")
}

/// What an upload carried, with the file on disk at its `.part` path.
#[derive(Default)]
struct Upload {
    title: Option<String>,
    kind: Option<String>,
    file: Option<(String, ProofKind, u64)>,
}

/// Attach a file to an issue as proof, in the issue's current attempt. Nobody in a role the
/// issue's status locks may attach proof, as for comments.
#[utoipa::path(
    post,
    path = "/api/issues/{id}/proof",
    params(("id" = String, Path, description = "Issue id, such as PL-1")),
    request_body(content = AttachProof, content_type = "multipart/form-data"),
    responses(
        (status = 201, body = Proof),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "An agent attached proof to another issue than its run's."),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody, description = "The issue's status is locked to the actor's role, or its attempt changed while the file uploaded."),
        (status = 413, body = ErrorBody, description = "The file is over the server's limit."),
        (status = 422, body = ErrorBody, description = "No title or file, a content type proof cannot be, or a kind that is not the content type's."),
    )
)]
async fn attach_proof(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<String>,
    multipart: Multipart,
) -> Result<(StatusCode, Json<Proof>), ApiError> {
    let key = Uuid::new_v4().to_string();
    let path = state.proof.dir.join(&key);
    let part = state.proof.dir.join(format!("{key}.part"));
    let attached = attach(&state, &actor, &id, multipart, &key, &path, &part).await;
    if attached.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
        let _ = tokio::fs::remove_file(&path).await;
    }
    attached.map(|proof| (StatusCode::CREATED, Json(proof)))
}

async fn attach(
    state: &AppState,
    actor: &Actor,
    id: &str,
    mut multipart: Multipart,
    key: &str,
    path: &FsPath,
    part: &FsPath,
) -> Result<Proof, ApiError> {
    let malformed = |error: axum::extract::multipart::MultipartError| {
        ApiError::Invalid(format!(
            "the upload is not valid multipart: {}",
            error.body_text()
        ))
    };
    // Before the body is read: a refused upload is not streamed to disk first, and the
    // proof belongs to the attempt that was current when it started, checked again below.
    let started = {
        let mut conn = state.pool.acquire().await?;
        let status = sqlx::query_scalar!(
            "SELECT status FROM issue WHERE id = $1 AND workspace_id = $2",
            id,
            actor.workspace_id,
        )
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| ApiError::IssueNotFound(id.to_string()))?;
        unlocked_attempt(&mut conn, actor, id, &status).await?
    };
    let mut upload = Upload::default();
    while let Some(mut field) = multipart.next_field().await.map_err(malformed)? {
        match field.name() {
            Some("title") => upload.title = Some(field.text().await.map_err(malformed)?),
            Some("kind") => upload.kind = Some(field.text().await.map_err(malformed)?),
            Some("file") if upload.file.is_none() => {
                // The part's content type, without parameters such as `; charset=utf-8`.
                let content_type = (field.content_type().unwrap_or_default())
                    .split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase();
                let kind = proof_kind(&content_type).ok_or_else(|| {
                    ApiError::Invalid(format!(
                        "proof cannot be `{content_type}`; it can be {}",
                        accepted()
                    ))
                })?;
                tokio::fs::create_dir_all(&state.proof.dir).await?;
                let mut file = tokio::fs::File::create(part).await?;
                let mut size: u64 = 0;
                while let Some(chunk) = field.chunk().await.map_err(malformed)? {
                    size += chunk.len() as u64;
                    if size > state.proof.max_bytes {
                        return Err(ApiError::TooLarge(format!(
                            "the file is over the limit of {} bytes",
                            state.proof.max_bytes
                        )));
                    }
                    file.write_all(&chunk).await?;
                }
                file.flush().await?;
                upload.file = Some((content_type, kind, size));
            }
            Some("file") => return Err(ApiError::Invalid("attach one file at a time".into())),
            _ => {}
        }
    }
    let title = (upload.title.as_deref().map(str::trim))
        .filter(|title| !title.is_empty())
        .ok_or_else(|| ApiError::Invalid("proof needs a title".into()))?
        .to_string();
    let (content_type, kind, size) = upload
        .file
        .ok_or_else(|| ApiError::Invalid("proof needs a file".into()))?;
    if let Some(declared) = upload.kind.as_deref().map(str::trim)
        && declared != kind.as_str()
    {
        return Err(ApiError::Invalid(format!(
            "`{content_type}` is {} proof, not `{declared}`",
            kind.as_str()
        )));
    }
    tokio::fs::rename(part, path).await?;
    // ponytail: a crash between this rename and the commit leaves a file no row names.
    // Upgrade path: a sweep that deletes stored keys with no `proof` row.
    let mut tx = state.pool.begin().await?;
    // Workspace before issue, as everywhere.
    lock_workspace(&mut tx, actor.workspace_id).await?;
    let status = sqlx::query_scalar!(
        "SELECT status FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
        id,
        actor.workspace_id,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::IssueNotFound(id.to_string()))?;
    let attempt = unlocked_attempt(&mut tx, actor, id, &status).await?;
    if attempt != started {
        return Err(ApiError::StaleAttempt(format!(
            "{id} moved from attempt {started} to attempt {attempt} while the proof uploaded, so it was not attached"
        )));
    }
    let row = sqlx::query!(
        "INSERT INTO proof (issue_id, attempt, kind, title, content_type, size, storage_key, actor_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id, created_at",
        id,
        attempt,
        kind.as_str(),
        title,
        content_type,
        size as i64,
        key,
        actor.id,
    )
    .fetch_one(&mut *tx)
    .await?;
    record_row_event(&mut tx, actor.workspace_id, id, actor.id, "proof").await?;
    tx.commit().await?;
    Ok(Proof {
        id: row.id,
        issue_id: id.to_string(),
        attempt,
        kind,
        title,
        content_type,
        size: size as i64,
        actor: actor.into(),
        created_at: row.created_at,
    })
}

/// The issue's attempt, once its status is checked against the actor's role.
async fn unlocked_attempt(
    conn: &mut sqlx::PgConnection,
    actor: &Actor,
    id: &str,
    status: &str,
) -> Result<i32, ApiError> {
    let (workflow, _) = active_workflow(conn, actor.workspace_id).await?;
    workflow.check_unlocked(status, actor.role)?;
    Ok(current_attempt(conn, id).await?)
}

/// An issue's proof, oldest first.
#[utoipa::path(
    get,
    path = "/api/issues/{id}/proof",
    params(("id" = String, Path, description = "Issue id, such as PL-1")),
    responses(
        (status = 200, body = Vec<Proof>),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
async fn list_proof(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<String>,
) -> Result<Json<Vec<Proof>>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM issue WHERE id = $1 AND workspace_id = $2) AS "exists!""#,
        id,
        actor.workspace_id,
    )
    .fetch_one(&mut *conn)
    .await?;
    if !exists {
        return Err(ApiError::IssueNotFound(id));
    }
    let rows = sqlx::query_as!(
        ProofRow,
        "SELECT p.id, p.issue_id, p.attempt, p.kind, p.title, p.content_type, p.size,
                a.id AS actor_id, a.name AS actor_name, a.role AS actor_role,
                a.kind AS actor_kind, p.created_at
         FROM proof p JOIN actor a ON a.id = p.actor_id
         WHERE p.issue_id = $1 ORDER BY p.created_at, p.id",
        id,
    )
    .fetch_all(&mut *conn)
    .await?;
    let proof = rows
        .into_iter()
        .map(Proof::try_from)
        .collect::<Result<_, _>>()?;
    Ok(Json(proof))
}

/// A proof file's bytes, with its content type. It is served so that a browser neither
/// guesses another type nor runs it. An agent's token gets 403: agents reach only
/// `/api/issues/...`.
#[utoipa::path(
    get,
    path = "/api/proof/{id}",
    params(("id" = Uuid, Path, description = "Proof id")),
    responses(
        (status = 200, description = "The file, with its stored content type."),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
    )
)]
async fn get_proof(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    // Another workspace's proof is not found, as if it did not exist.
    let row = sqlx::query!(
        "SELECT p.content_type, p.storage_key
         FROM proof p JOIN issue i ON i.id = p.issue_id
         WHERE p.id = $1 AND i.workspace_id = $2",
        id,
        actor.workspace_id,
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::ProofNotFound(id))?;
    // ponytail: the whole file is read into memory, so a response holds up to the size
    // limit. Upgrade path: stream it from disk.
    let bytes = tokio::fs::read(state.proof.dir.join(&row.storage_key)).await?;
    Ok((
        [
            (CONTENT_TYPE, row.content_type),
            (X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            (CONTENT_SECURITY_POLICY, "sandbox".to_string()),
        ],
        bytes,
    ))
}
