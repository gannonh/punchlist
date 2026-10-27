//! `/api/workspaces/{id}/invites` (PL-169): a person of a workspace invites a GitHub user, whose
//! next sign-in with GitHub (`github_sign_in`) makes them a person there.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use punchlist_api::{CreateInvite, ErrorBody, Invite};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::actors::person_only;
use crate::auth::Actor;
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_invite, list_invites))
        .routes(routes!(revoke_invite))
}

fn check_workspace(actor: &Actor, id: Uuid) -> Result<(), ApiError> {
    if id != actor.workspace_id {
        return Err(ApiError::WorkspaceNotFound(id));
    }
    person_only(actor)
}

/// GitHub's shape for a new login: letters, digits and single hyphens, neither first nor
/// last, at most 39 characters.
fn github_login(login: &str) -> bool {
    login.len() <= 39
        && (login.split('-'))
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

/// Invite a GitHub user to the workspace. Person token.
#[utoipa::path(
    post,
    path = "/api/workspaces/{id}/invites",
    params(("id" = Uuid, Path, description = "The token's workspace")),
    request_body = CreateInvite,
    responses(
        (status = 201, body = Invite),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "Not a person's token."),
        (status = 404, body = ErrorBody, description = "Not the token's workspace."),
        (status = 409, body = ErrorBody, description = "The login is a member already, or has an open invite."),
        (status = 422, body = ErrorBody, description = "Not a GitHub login."),
    )
)]
async fn create_invite(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
    Json(request): Json<CreateInvite>,
) -> Result<(StatusCode, Json<Invite>), ApiError> {
    check_workspace(&actor, id)?;
    let login = request.github_login.trim();
    if !github_login(login) {
        return Err(ApiError::Invalid(format!(
            "`{login}` is not a GitHub login: letters, digits and single hyphens, at most 39 characters"
        )));
    }
    let member = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM actor a JOIN account acc ON acc.id = a.account_id
                          WHERE a.workspace_id = $1 AND lower(acc.github_login) = lower($2))
           AS "member!""#,
        id,
        login,
    )
    .fetch_one(&state.pool)
    .await?;
    if member {
        return Err(ApiError::Conflict(format!(
            "{login} is a member of this workspace already"
        )));
    }
    let invite = sqlx::query_as!(
        Invite,
        r#"WITH created AS (
               INSERT INTO invite (workspace_id, github_login, invited_by)
               VALUES ($1, $2::text, $3)
               ON CONFLICT (workspace_id, github_login)
                   WHERE accepted_at IS NULL AND revoked_at IS NULL DO NOTHING
               RETURNING id, github_login, invited_by, created_at
           )
           SELECT c.id, c.github_login::text AS "github_login!", a.name AS invited_by,
                  c.created_at
           FROM created c JOIN actor a ON a.id = c.invited_by"#,
        id,
        login,
        actor.id,
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::Conflict(format!("{login} has an open invite already")))?;
    Ok((StatusCode::CREATED, Json(invite)))
}

/// The workspace's open invites, oldest first. Person token.
#[utoipa::path(
    get,
    path = "/api/workspaces/{id}/invites",
    params(("id" = Uuid, Path, description = "The token's workspace")),
    responses(
        (status = 200, body = Vec<Invite>),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "Not a person's token."),
        (status = 404, body = ErrorBody, description = "Not the token's workspace."),
    )
)]
async fn list_invites(
    State(state): State<AppState>,
    actor: Actor,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<Invite>>, ApiError> {
    check_workspace(&actor, id)?;
    let invites = sqlx::query_as!(
        Invite,
        r#"SELECT i.id, i.github_login::text AS "github_login!", a.name AS invited_by,
                  i.created_at
           FROM invite i JOIN actor a ON a.id = i.invited_by
           WHERE i.workspace_id = $1 AND i.accepted_at IS NULL AND i.revoked_at IS NULL
           ORDER BY i.created_at, i.id"#,
        id,
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(invites))
}

/// Revoke an open invite: the GitHub user's sign-in no longer joins the workspace. Person
/// token.
#[utoipa::path(
    delete,
    path = "/api/workspaces/{id}/invites/{invite}",
    params(
        ("id" = Uuid, Path, description = "The token's workspace"),
        ("invite" = Uuid, Path, description = "The invite's id"),
    ),
    responses(
        (status = 204),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "Not a person's token."),
        (status = 404, body = ErrorBody, description = "Not the token's workspace, or no open invite of that id in it."),
    )
)]
async fn revoke_invite(
    State(state): State<AppState>,
    actor: Actor,
    Path((id, invite)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    check_workspace(&actor, id)?;
    let revoked = sqlx::query!(
        "UPDATE invite SET revoked_at = now()
         WHERE id = $1 AND workspace_id = $2 AND accepted_at IS NULL AND revoked_at IS NULL",
        invite,
        id,
    )
    .execute(&state.pool)
    .await?
    .rows_affected();
    if revoked == 0 {
        return Err(ApiError::InviteNotFound(invite));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::github_login;

    #[test]
    fn a_github_login_is_letters_digits_and_single_hyphens() {
        let forty = "a".repeat(40);
        let shaped: Vec<bool> = ["octo-cat", "Octocat", "a1-b2-c3", &forty[1..]]
            .into_iter()
            .chain(["octo--cat", &forty, "-x", "x-", "", "octo_cat", "octo.cat"])
            .map(github_login)
            .collect();
        assert_eq!(
            shaped,
            [
                true, true, true, true, false, false, false, false, false, false, false
            ]
        );
    }
}
