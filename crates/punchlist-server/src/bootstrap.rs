use sqlx::PgPool;
use uuid::Uuid;

use crate::hash_token;

/// What `punchlist-server bootstrap` creates: one workspace, its repository and one person.
#[derive(Debug, Clone)]
pub struct Bootstrap {
    pub workspace_name: String,
    pub issue_prefix: String,
    pub repository_owner: String,
    pub repository_name: String,
    pub person_name: String,
}

#[derive(Debug, Clone)]
pub struct Bootstrapped {
    pub workspace_id: Uuid,
    pub actor_id: Uuid,
    /// The person's bearer token. Shown once; only its hash is stored.
    pub token: String,
}

pub async fn bootstrap(pool: &PgPool, input: &Bootstrap) -> anyhow::Result<Bootstrapped> {
    let token = crate::auth::new_token();
    let mut tx = pool.begin().await?;
    let workspace_id = sqlx::query_scalar!(
        "INSERT INTO workspace (name, issue_prefix) VALUES ($1, $2) RETURNING id",
        input.workspace_name,
        input.issue_prefix,
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!(
        "INSERT INTO repository (workspace_id, owner, name) VALUES ($1, $2, $3)",
        workspace_id,
        input.repository_owner,
        input.repository_name,
    )
    .execute(&mut *tx)
    .await?;
    let actor_id = sqlx::query_scalar!(
        "INSERT INTO actor (workspace_id, name, role, token_hash)
         VALUES ($1, $2, 'person', $3) RETURNING id",
        workspace_id,
        input.person_name,
        hash_token(&token),
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Bootstrapped {
        workspace_id,
        actor_id,
        token,
    })
}
