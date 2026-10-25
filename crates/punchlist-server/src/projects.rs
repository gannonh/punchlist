//! `/api/projects`: a workspace's projects and their milestones, which issues are placed in.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use punchlist_api::{
    CreateMilestone, CreateProject, ErrorBody, Milestone, Project, ProjectList, UpdateMilestone,
    UpdateProject,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::actors::person_only;
use crate::auth::Actor;
use crate::events::record_row_event;
use crate::workflow::lock_workspace;
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_project, list_projects))
        .routes(routes!(create_milestone))
        .routes(routes!(update_project))
        .routes(routes!(update_milestone))
}

const MAX_NAME_CHARS: usize = 100;

/// A project's or a milestone's name, trimmed and 1 to 100 characters.
fn parse_name<'a>(raw: &'a str, what: &str) -> Result<&'a str, ApiError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(ApiError::Invalid(format!("a {what} needs a name")));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(ApiError::Invalid(format!(
            "a {what} name is at most {MAX_NAME_CHARS} characters"
        )));
    }
    Ok(name)
}

/// List the workspace's projects, oldest first, each with its milestones, oldest first.
#[utoipa::path(
    get,
    path = "/api/projects",
    responses((status = 200, body = ProjectList), (status = 401, body = ErrorBody))
)]
pub(crate) async fn list_projects(
    State(state): State<AppState>,
    actor: Actor,
) -> Result<Json<ProjectList>, ApiError> {
    let mut projects: Vec<Project> = sqlx::query!(
        "SELECT id, name FROM project WHERE workspace_id = $1 ORDER BY created_at, id",
        actor.workspace_id,
    )
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|row| Project {
        id: row.id,
        name: row.name,
        milestones: Vec::new(),
    })
    .collect();
    let milestones = sqlx::query!(
        "SELECT m.project_id, m.id, m.name, m.pass_when
         FROM milestone m JOIN project p ON p.id = m.project_id
         WHERE p.workspace_id = $1 ORDER BY m.created_at, m.id",
        actor.workspace_id,
    )
    .fetch_all(&state.pool)
    .await?;
    for row in milestones {
        if let Some(project) = projects.iter_mut().find(|p| p.id == row.project_id) {
            project.milestones.push(Milestone {
                id: row.id,
                name: row.name,
                pass_when: row.pass_when,
            });
        }
    }
    Ok(Json(ProjectList { projects }))
}

/// Create a project in the workspace.
#[utoipa::path(
    post,
    path = "/api/projects",
    request_body = CreateProject,
    responses(
        (status = 201, body = Project),
        (status = 401, body = ErrorBody),
        (status = 422, body = ErrorBody, description = "An empty or long name, or one the workspace already has."),
    )
)]
pub(crate) async fn create_project(
    State(state): State<AppState>,
    actor: Actor,
    Json(request): Json<CreateProject>,
) -> Result<(StatusCode, Json<Project>), ApiError> {
    let name = parse_name(&request.name, "project")?;
    let id = sqlx::query_scalar!(
        "INSERT INTO project (workspace_id, name) VALUES ($1, $2)
         ON CONFLICT (workspace_id, name) DO NOTHING RETURNING id",
        actor.workspace_id,
        name,
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::Invalid(format!("a project named {name} already exists")))?;
    Ok((
        StatusCode::CREATED,
        Json(Project {
            id,
            name: name.to_string(),
            milestones: Vec::new(),
        }),
    ))
}

/// Create a milestone in one of the workspace's projects.
#[utoipa::path(
    post,
    path = "/api/projects/{id}/milestones",
    params(("id" = Uuid, Path, description = "The project's id")),
    request_body = CreateMilestone,
    responses(
        (status = 201, body = Milestone),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 422, body = ErrorBody, description = "An empty or long name, or one the project already has."),
    )
)]
async fn create_milestone(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(request): Json<CreateMilestone>,
) -> Result<(StatusCode, Json<Milestone>), ApiError> {
    let name = parse_name(&request.name, "milestone")?;
    // Projects are never deleted, so it is still there for the insert.
    let project = sqlx::query_scalar!(
        "SELECT name FROM project WHERE id = $1 AND workspace_id = $2",
        id,
        actor.workspace_id,
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::ProjectNotFound(id))?;
    let pass_when = request.pass_when.trim();
    let milestone_id = sqlx::query_scalar!(
        "INSERT INTO milestone (project_id, name, pass_when) VALUES ($1, $2, $3)
         ON CONFLICT (project_id, name) DO NOTHING RETURNING id",
        id,
        name,
        pass_when,
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| {
        ApiError::Invalid(format!(
            "project {project} already has a milestone named {name}"
        ))
    })?;
    Ok((
        StatusCode::CREATED,
        Json(Milestone {
            id: milestone_id,
            name: name.to_string(),
            pass_when: pass_when.to_string(),
        }),
    ))
}

fn nothing_to_change(fields: &str) -> ApiError {
    ApiError::Invalid(format!("nothing to change: send {fields}"))
}

/// `refused` for a name another row already has, else the database's error.
fn taken(error: sqlx::Error, refused: impl FnOnce() -> String) -> ApiError {
    if error
        .as_database_error()
        .is_some_and(|e| e.is_unique_violation())
    {
        ApiError::Invalid(refused())
    } else {
        error.into()
    }
}

/// Writes one `project` event per issue `issues` returns, so open lists show a new name.
async fn record_renamed(
    tx: &mut sqlx::PgConnection,
    actor: &Actor,
    issues: Vec<String>,
) -> Result<(), ApiError> {
    for issue in issues {
        record_row_event(tx, actor.workspace_id, &issue, actor.id, "project").await?;
    }
    Ok(())
}

/// Rename a project. Each of its issues gets a `project` event. Person token.
#[utoipa::path(
    patch,
    path = "/api/projects/{id}",
    params(("id" = Uuid, Path, description = "The project's id")),
    request_body = UpdateProject,
    responses(
        (status = 200, body = Project),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 422, body = ErrorBody, description = "No name, an empty or long one, or one the workspace already has."),
    )
)]
async fn update_project(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateProject>,
) -> Result<Json<Project>, ApiError> {
    person_only(&actor)?;
    let name = parse_name(
        request
            .name
            .as_deref()
            .ok_or_else(|| nothing_to_change("name"))?,
        "project",
    )?;
    let mut tx = state.pool.begin().await?;
    lock_workspace(&mut tx, actor.workspace_id).await?;
    let old = sqlx::query_scalar!(
        "SELECT name FROM project WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
        id,
        actor.workspace_id,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::ProjectNotFound(id))?;
    if old != name {
        sqlx::query!("UPDATE project SET name = $2 WHERE id = $1", id, name)
            .execute(&mut *tx)
            .await
            .map_err(|e| taken(e, || format!("a project named {name} already exists")))?;
        let issues = sqlx::query_scalar!(
            "SELECT id FROM issue WHERE project_id = $1 ORDER BY number",
            id
        )
        .fetch_all(&mut *tx)
        .await?;
        record_renamed(&mut tx, &actor, issues).await?;
    }
    let milestones = sqlx::query_as!(
        Milestone,
        "SELECT id, name, pass_when FROM milestone WHERE project_id = $1 ORDER BY created_at, id",
        id,
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(Project {
        id,
        name: name.to_string(),
        milestones,
    }))
}

/// Rename a milestone or set its `pass_when`, which a later Linear import then keeps. A rename
/// gives each of its issues a `project` event. Person token.
#[utoipa::path(
    patch,
    path = "/api/projects/{id}/milestones/{milestone_id}",
    params(
        ("id" = Uuid, Path, description = "The project's id"),
        ("milestone_id" = Uuid, Path, description = "The milestone's id, in that project"),
    ),
    request_body = UpdateMilestone,
    responses(
        (status = 200, body = Milestone),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 422, body = ErrorBody, description = "Nothing to change, an empty or long name, or one the project already has."),
    )
)]
async fn update_milestone(
    State(state): State<AppState>,
    actor: Actor,
    Path((id, milestone_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<UpdateMilestone>,
) -> Result<Json<Milestone>, ApiError> {
    person_only(&actor)?;
    if request.name.is_none() && request.pass_when.is_none() {
        return Err(nothing_to_change("name or pass_when"));
    }
    let name = (request.name.as_deref())
        .map(|name| parse_name(name, "milestone"))
        .transpose()?;
    let pass_when = request.pass_when.as_deref().map(str::trim);
    let mut tx = state.pool.begin().await?;
    lock_workspace(&mut tx, actor.workspace_id).await?;
    let project = sqlx::query_scalar!(
        "SELECT name FROM project WHERE id = $1 AND workspace_id = $2",
        id,
        actor.workspace_id,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::ProjectNotFound(id))?;
    let old = sqlx::query_scalar!(
        "SELECT name FROM milestone WHERE id = $1 AND project_id = $2 FOR UPDATE",
        milestone_id,
        id,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ApiError::MilestoneNotFound(milestone_id))?;
    let name = name.unwrap_or(old.as_str());
    let milestone = sqlx::query_as!(
        Milestone,
        "UPDATE milestone SET name = $2, pass_when = coalesce($3, pass_when),
                pass_when_edited = pass_when_edited OR $3 IS NOT NULL
         WHERE id = $1 RETURNING id, name, pass_when",
        milestone_id,
        name,
        pass_when,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        taken(e, || {
            format!("project {project} already has a milestone named {name}")
        })
    })?;
    if old != name {
        let issues = sqlx::query_scalar!(
            "SELECT id FROM issue WHERE milestone_id = $1 ORDER BY number",
            milestone_id
        )
        .fetch_all(&mut *tx)
        .await?;
        record_renamed(&mut tx, &actor, issues).await?;
    }
    tx.commit().await?;
    Ok(Json(milestone))
}
