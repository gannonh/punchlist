# 7. Use axum, sqlx, a Postgres job table and clap

Date: 2026-10-07. Status: accepted.

## Context

ADR 0003 left the HTTP server, database access and job queue to the first slice that needs them. The first Gate 1 slice needs all of them, so they are decided together. Kata Symphony, which the runner copies from (ADR 0005), already uses tokio, axum 0.8, clap 4, serde, reqwest, thiserror, anyhow and tracing.

**HTTP server.** axum is built on tokio and tower, and Symphony's HTTP server and tests already use it. Actix Web and Rocket would add a second framework beside the copied code.

**Postgres access and migrations.**

- sqlx runs plain SQL, checks each `query!` against the schema at compile time, and runs migrations from SQL files. Offline mode stores the query metadata in `.sqlx/`, so a build does not need a live database.
- Diesel builds queries in Rust and is synchronous unless `diesel-async` is added.
- SeaORM is an ORM on top of sqlx.

Punchlist's hard queries are the transition transaction, lease claims with compare-and-set and `SELECT ... FOR UPDATE SKIP LOCKED`. Those read most clearly as SQL.

**Background jobs.** The server has a few kinds of job in v1: process a GitHub webhook delivery, re-evaluate gates after new evidence arrives, and run a Linear import. A job must be enqueued in the same transaction as the event that caused it, so a crash cannot record one without the other. Postgres-backed queue crates exist, such as `graphile_worker` (a Rust port of the Node library) and `awa` (0.6), and each brings its own schema and worker runtime. Runner claims cannot use them anyway: a runner claims over HTTP with a lease and an attempt number (ADR 0005), so the server needs its own lease code.

**CLI parsing.** clap with derive is the common choice and Symphony uses it.

## Decision

- Async runtime: tokio. HTTP: axum 0.8 with tower-http for static files, tracing and timeouts.
- Postgres: sqlx with the `postgres` and `migrate` features. Queries use the compile-time checked macros, and `.sqlx/` is committed so builds and CI work without a database. Migrations are SQL files in `crates/punchlist-server/migrations/`, embedded with `sqlx::migrate!` and run when the server starts.
- Jobs: a `job` table in Postgres, claimed with `FOR UPDATE SKIP LOCKED`, with a lease, an attempt count and capped backoff. Code enqueues a job inside the transaction that records its event. The worker runs inside the server process.
- Delivery is at least once, so every job is idempotent. A job that only writes to Postgres commits its writes and its completion in one transaction. A job with an outside effect, such as a call to GitHub or Linear, carries an idempotency key and records each effect under that key, so a rerun skips what already happened.
- CLI: clap 4 with derive, for `pl` and for the server's own flags.
- Shared crates: serde and serde_json; thiserror in library crates and anyhow in binaries; tracing with tracing-subscriber; reqwest as the HTTP client.
- Workflow parsing in `punchlist-core`: the `toml` crate with serde, with spans so a validation error names the line (ADR 0001), and schemars to generate the published JSON Schema.
- MCP: rmcp, the official Rust SDK for the Model Context Protocol.

## Consequences

- The copied Symphony code compiles against the same libraries it was written for.
- SQL changes that break a query fail the build. A developer who changes a query regenerates `.sqlx/` with `cargo sqlx prepare`, and CI fails when it is stale.
- The job queue is one table and a worker loop that Punchlist owns and tests, and it shares the lease pattern with runner claims. Features such as cron schedules and an admin UI are not included; none is in the v1 requirements.
- A job and its event commit together, so a crash never loses webhook processing or gate re-evaluation. A crash mid-job runs the job again, and idempotency keeps that rerun from doubling its effects.
