# Punchlist

Punchlist is an open-source issue tracker where coding agents do the work and the tracker enforces how the work gets done. It is for solo developers and small teams that run several coding agents in parallel.

- Product spec: `docs/product/prd.md`.
- Decisions: `docs/adr/`. Read the relevant ADR before changing what it decided.
- Work: the Linear project **Punchlist** in the Kata-sh team, starting with milestone **Gate 0: Spec, stack and prototype**.

## Stack

- Backend: Rust in one Cargo workspace (ADR 0003). `punchlist-core` is pure and holds the workflow types and parser, the gates and the transition rules. The server, runner, `pl` CLI and MCP server depend on it. Postgres is the store.
- Rust libraries (ADR 0007): tokio and axum 0.8 with tower-http; sqlx for Postgres, with compile-time checked queries, committed `.sqlx/` offline data and SQL migrations run at server start; a Postgres `job` table claimed with `FOR UPDATE SKIP LOCKED` for background jobs; clap 4 for CLIs; `toml` with spans and schemars for the workflow file; rmcp for MCP; serde, thiserror, anyhow, tracing and reqwest.
- Runner (ADR 0005): `punchlist-runner` holds modules copied from Kata Symphony's orchestrator in `~/dev/kata-symphony/apps/symphony` at commit `e7c160f5`, each marked with its source path. The runner has no database; the server holds runs, attempts and leases.
- Frontend: a web app in TypeScript, React and Vite, served by the Rust server (ADR 0004). TanStack Router and TanStack Query; server state with optimistic field edits, pending transitions and a Server-Sent Events stream from the event log (ADR 0006). Tailwind with shadcn/ui, as in Groundwork.
- API types (ADR 0008): `punchlist-api` types derive utoipa schemas, utoipa-axum builds the OpenAPI document from the router, and openapi-typescript and openapi-fetch give the web app a typed client. `pnpm gen:api` regenerates `web/src/api/`; CI fails when it is stale.
- Data model (ADR 0010): one GitHub repository per workspace in v1. The active workflow is `.punchlist/workflow.toml` on that repository's default branch.
- License: MIT or Apache-2.0 (ADR 0002). Every crate sets `license = "MIT OR Apache-2.0"`.

### Layout and commands

The target layout (ADR 0009). A crate or directory is created by the first slice that needs it.

```text
Cargo.toml, rust-toolchain.toml, package.json, .sqlx/
crates/punchlist-core/     pure workflow, gates and transition rules; builds for wasm32
crates/punchlist-api/      API request and response types
crates/punchlist-server/   axum server, migrations/, jobs; binary punchlist-server
crates/punchlist-client/   Rust API client
crates/punchlist-runner/   runner from Symphony's orchestrator
crates/punchlist-mcp/      MCP server over stdio
crates/punchlist-cli/      binary pl
web/                       React app; src/api/ is generated
.punchlist/                Punchlist's own workflow and prompts
docker-compose.yml         Postgres for development; the self-host stack
```

- `pnpm dev`: Postgres in Docker, the server, and Vite proxying `/api`.
- `pnpm check`: rustfmt, clippy with `-D warnings`, `cargo test`, the web typecheck, lint and unit tests, and stale-generated-file checks. Run it before a pull request.
- `pnpm gen:api`: regenerate the OpenAPI document and TypeScript types.
- `pnpm build`: build `web/dist`, then the release binaries.
- `pnpm e2e`: Playwright against a built server.

Keep setup, scripts and layout documented here as slices land.

## Rules for the code

- The workflow lives in `.punchlist/workflow.toml`, and prompts live in `.punchlist/prompts/` (ADR 0001). Never put workflow rules in a prompt.
- A gate is a pure function in `punchlist-core` over recorded evidence. An agent's claim that a gate passed is never evidence.
- The server checks roles, locks and gates for a transition in one transaction, and records the actor, the workflow version and each gate result.
- Parse external data where it enters: workflow files, GitHub webhooks, Linear imports, agent requests. Trust typed data inside.
- Text from outside the team reaches an agent only inside a fence that the text cannot close.
- Runs are idempotent per issue and attempt. A claim is a lease, and a runner restart never loses or duplicates a run.
