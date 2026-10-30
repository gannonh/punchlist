use sqlx::PgPool;
use uuid::Uuid;

use crate::db;
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
    pub account_id: Uuid,
    /// The account's bearer token. Shown once; only its hash is stored.
    pub token: String,
}

/// Creates the workspace, its repository and its person, in a new account named as the
/// person, with a new token.
pub async fn bootstrap(pool: &PgPool, input: &Bootstrap) -> anyhow::Result<Bootstrapped> {
    let token = crate::auth::new_token();
    let mut tx = db::begin(pool).await?;
    let account_id = sqlx::query_scalar!(
        "INSERT INTO account (name, token_hash) VALUES ($1, $2) RETURNING id",
        input.person_name,
        hash_token(&token),
    )
    .fetch_one(&mut *tx)
    .await?;
    let (workspace_id, actor_id) = create(&mut tx, input, account_id).await?;
    tx.commit().await?;
    Ok(Bootstrapped {
        workspace_id,
        actor_id,
        account_id,
        token,
    })
}

/// `bootstrap --account`: creates the workspace, its repository and its person in an existing
/// account, whose token reaches it. Answers the workspace and the person.
pub async fn join(
    pool: &PgPool,
    account_id: Uuid,
    input: &Bootstrap,
) -> anyhow::Result<(Uuid, Uuid)> {
    let mut tx = db::begin(pool).await?;
    sqlx::query_scalar!("SELECT id FROM account WHERE id = $1", account_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no account {account_id}"))?;
    let created = create(&mut tx, input, account_id).await?;
    tx.commit().await?;
    Ok(created)
}

async fn create(
    tx: &mut sqlx::PgConnection,
    input: &Bootstrap,
    account_id: Uuid,
) -> anyhow::Result<(Uuid, Uuid)> {
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
        "INSERT INTO actor (workspace_id, name, role, account_id)
         VALUES ($1, $2, 'person', $3) RETURNING id",
        workspace_id,
        input.person_name,
        account_id,
    )
    .fetch_one(&mut *tx)
    .await?;
    Ok((workspace_id, actor_id))
}
