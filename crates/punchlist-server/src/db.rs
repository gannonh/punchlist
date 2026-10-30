//! Transactions on the pool. Begin each one with `begin`, never `Pool::begin` (clippy refuses
//! it, `clippy.toml`).

use sqlx::{PgPool, Postgres, Transaction};

/// `pool.begin()`, run on a task of its own so that a caller dropped meanwhile cannot cut it
/// short. sqlx 0.9.0 counts a transaction only once `BEGIN` is answered, so a `begin` dropped
/// while it waits for the answer, as a request is when its client goes away (PL-178), puts
/// the connection back in the pool inside that transaction. Every later statement on that
/// connection then joins it, and is lost when something rolls it back: a write answered 204
/// that no other connection ever sees. Run to the end, the `Transaction` exists and its drop
/// rolls it back.
pub async fn begin(pool: &PgPool) -> sqlx::Result<Transaction<'static, Postgres>> {
    let pool = pool.clone();
    #[expect(
        clippy::disallowed_methods,
        reason = "the one call, made uncancellable"
    )]
    let begin = tokio::spawn(async move { pool.begin().await });
    begin.await.expect("beginning a transaction does not panic")
}

#[cfg(test)]
mod tests {
    use super::begin;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use sqlx::{Connection, PgConnection, PgPool};
    use std::future::{Future, poll_fn};
    use std::pin::Pin;
    use std::task::Poll;

    /// Polls `future` `times` times, or until it is ready, then drops it.
    async fn cut_short<F: Future>(future: F, times: usize) {
        let mut future: Pin<Box<F>> = Box::pin(future);
        for _ in 0..times {
            if poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx)))
                .await
                .is_ready()
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    }

    async fn committed(connect: &PgConnectOptions) -> i64 {
        let mut other = PgConnection::connect_with(connect).await.unwrap();
        sqlx::query_scalar("SELECT count(*) FROM t")
            .fetch_one(&mut other)
            .await
            .unwrap()
    }

    async fn one_connection(options: PgPoolOptions, connect: &PgConnectOptions) -> PgPool {
        let pool = options
            .max_connections(1)
            .connect_with(connect.clone())
            .await
            .unwrap();
        sqlx::query("CREATE TABLE t (n int)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    /// Why `begin` exists: sqlx's own, dropped while `BEGIN` is unanswered, leaves the pool's
    /// connection in the transaction, and an insert after it is seen by no other connection.
    /// When this fails, sqlx is fixed and `begin` can go.
    #[sqlx::test]
    async fn sqlx_begin_cut_short_leaves_the_connection_in_a_transaction(
        options: PgPoolOptions,
        connect: PgConnectOptions,
    ) {
        let pool = one_connection(options, &connect).await;
        let mut conn = pool.acquire().await.unwrap();
        // One poll sends `BEGIN`; its answer is not read yet.
        #[expect(
            clippy::disallowed_methods,
            reason = "the call this test shows is unsafe"
        )]
        cut_short(conn.begin(), 1).await;
        drop(conn);
        sqlx::query("INSERT INTO t VALUES (1)")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(committed(&connect).await, 0);
    }

    #[sqlx::test]
    async fn a_begin_cut_short_at_any_point_leaves_no_transaction(
        options: PgPoolOptions,
        connect: PgConnectOptions,
    ) {
        let pool = one_connection(options, &connect).await;
        for times in 1..=20_i64 {
            cut_short(begin(&pool), times as usize).await;
            sqlx::query("INSERT INTO t VALUES (1)")
                .execute(&pool)
                .await
                .unwrap();
            assert_eq!(committed(&connect).await, times, "cut after {times} polls");
        }
    }
}
