//! `/api/projects`: a workspace's projects and their milestones, which issues are placed in.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use punchlist_api::{CreateMilestone, CreateProject, ErrorBody, Milestone, Project, ProjectList};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::Actor;
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_project, list_projects))
        .routes(routes!(create_milestone))
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
async fn list_projects(
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
async fn create_project(
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
