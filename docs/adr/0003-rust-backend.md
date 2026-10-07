# 3. Write the backend in Rust

Date: 2026-10-07. Status: accepted.

## Context

Gannon wants Rust for as much of the backend as possible. Three parts of Punchlist need the same rules. The server checks every transition against the workflow. The runner decides which issues it may dispatch and which prompt each status uses. The `pl` CLI validates a workflow file before it is committed. One implementation of those rules avoids three copies drifting apart.

Kata Symphony's orchestrator is the most mature code from the earlier projects. It is Rust: about 85,000 lines with roughly 1,150 tests. It already claims issues, retries with capped backoff, reconciles state against the tracker, detects stalled runs, and manages a worktree per issue. It uses tokio and axum.

Groundwork is TypeScript on TanStack Start with Drizzle. Its conventions do not carry over to a Rust backend.

## Decision

- The server, runner, `pl` CLI and MCP server are Rust, in one Cargo workspace. Postgres is the store.
- `punchlist-core` is a pure crate with no I/O. It holds the workflow types and parser, the gates as functions over recorded evidence, and the transition rules. Every other crate depends on it.
- The runner starts from Kata Symphony's orchestrator, adapted to claim work from the Punchlist server.
- Library choices for HTTP, database access and the job queue are made in the first slice that needs them.
- The frontend is a separate decision, made after a Gate 0 spike that builds the issue list both in GPUI and as a web app.

## Consequences

- Server, runner and CLI enforce the same rules because they call the same code.
- `punchlist-core` compiles to WebAssembly, so either frontend can validate a workflow or preview a gate in the client.
- Symphony's tested orchestration logic moves in as code rather than as ideas to port.
- Rust builds are slower than TypeScript ones, which lengthens agent edit-and-check loops. Splitting the workspace into small crates and checking with `cargo check` keeps the loop short.
- Groundwork and Punchlist no longer share a stack, so conventions and tooling are set up separately.
