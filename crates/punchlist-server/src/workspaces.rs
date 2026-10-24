//! The workspaces an account belongs to (ADR 0011), and the workspace-level routes served
//! again under `/api/workspaces/{id}/`, where the path names the workspace.

use axum::Json;
use axum::extract::State;
use axum::routing::{MethodRouter, get, post};
use punchlist_api::{ErrorBody, Workspace};
use utoipa::openapi::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::Actor;
use crate::{
    ApiError, AppState, actors, inbox, issues, projects, pull_requests, runners, search, workflow,
};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(list_workspaces))
}

/// The workspace-level routes a person calls, each also served at
/// `/api/workspaces/{id}<route>`. The extractor takes the workspace from that path; the form
/// without it answers for the account's only workspace.
fn workspace_routes() -> [(&'static str, MethodRouter<AppState>); 10] {
    [
        (
            "/issues",
            get(issues::list_issues).post(issues::create_issue),
        ),
        (
            "/projects",
            get(projects::list_projects).post(projects::create_project),
        ),
        ("/workflow", get(workflow::show_workflow)),
        (
            "/actors",
            get(actors::list_actors).post(actors::create_agent),
        ),
        (
            "/runners",
            get(runners::list_runners).post(runners::register_runner),
        ),
        ("/pull-requests", get(pull_requests::open_pull_requests)),
        (
            "/pull-requests/unlinked",
            get(pull_requests::unlinked_pull_requests),
        ),
        ("/search", get(search::search)),
        ("/inbox", get(inbox::inbox)),
        ("/inbox/read", post(inbox::mark_read)),
    ]
}

/// `/api/workspaces/{id}<route>` as its `<route>`, for a route of `workspace_routes`.
pub fn workspace_route(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/api/workspaces/")?;
    let route = &rest[rest.find('/')?..];
    workspace_routes()
        .iter()
        .any(|(known, _)| *known == route)
        .then_some(route)
}

/// `router` and `api` with each workspace route added under `/api/workspaces/{id}`, with the
/// same handlers and documentation and an `id` path parameter.
pub fn with_workspace_routes(
    mut router: axum::Router<AppState>,
    api: &mut OpenApi,
) -> axum::Router<AppState> {
    let id: utoipa::openapi::RefOr<utoipa::openapi::path::Parameter> =
        serde_json::from_value(serde_json::json!({
            "name": "id",
            "in": "path",
            "description": "A workspace the token is in",
            "required": true,
            "schema": {"type": "string", "format": "uuid"}
        }))
        .expect("a valid parameter");
    for (route, handlers) in workspace_routes() {
        let path = format!("/api/workspaces/{{id}}{route}");
        router = router.route(&path, handlers);
        let mut item = api.paths.paths[&format!("/api{route}")].clone();
        for operation in [&mut item.get, &mut item.post].into_iter().flatten() {
            if let Some(name) = &mut operation.operation_id {
                name.push_str("_in_workspace");
            }
            (operation.parameters.get_or_insert_default()).insert(0, id.clone());
        }
        api.paths.paths.insert(path, item);
    }
    router
}

/// The workspaces the token's account belongs to, oldest first. An agent's or runner's token
/// answers its own workspace.
#[utoipa::path(
    get,
    path = "/api/workspaces",
    responses((status = 200, body = Vec<Workspace>), (status = 401, body = ErrorBody))
)]
async fn list_workspaces(
    State(state): State<AppState>,
    actor: Actor,
) -> Result<Json<Vec<Workspace>>, ApiError> {
    let workspaces = sqlx::query_as!(
        Workspace,
        r#"SELECT w.id, w.name, w.issue_prefix, r.owner || '/' || r.name AS "repository!"
           FROM workspace w JOIN repository r ON r.workspace_id = w.id
           WHERE EXISTS (SELECT 1 FROM actor a WHERE a.workspace_id = w.id
                         AND (a.account_id = $1 OR a.id = $2))
           ORDER BY w.created_at, w.id"#,
        actor.account.as_ref().map(|account| account.id),
        actor.id,
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(workspaces))
}
