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

Exists today: `punchlist-core`, `punchlist-api`, `punchlist-server`, `punchlist-client`, `punchlist-runner`, `punchlist-mcp`, `punchlist-cli` and `.punchlist/`. There is no `web/` yet, so `pnpm dev` has no Vite and `pnpm gen:api`, `pnpm build` and `pnpm e2e` do not exist yet.

- Rust comes from rustup, which reads `rust-toolchain.toml`. Both machines get it from devops' `bin/machine-setup`.
- `pnpm dev`: Postgres in Docker on `127.0.0.1:5433`, then the server on `127.0.0.1:7878`. Local defaults for `DATABASE_URL` and `PUNCHLIST_BIND` are in `scripts/env.sh`; they are not secrets.
- `pnpm check`: rustfmt, clippy with `-D warnings`, `punchlist-core` for wasm32, `cargo test` (starts Postgres; the sqlx tests need it), and the `.sqlx/` staleness check. Run it before a pull request. CI runs the same script.
- `pnpm db:prepare`: after changing a query or a migration, migrate the dev database and regenerate `.sqlx/`. Needs `sqlx-cli` (`cargo install sqlx-cli --no-default-features --features postgres,rustls`).
- `punchlist-server bootstrap --person <name>` creates a workspace, its repository and one person, and prints a `pl` config with the person's token.
- `pl` reads `server_url` and `token` from `~/.config/punchlist/config.toml`, or from the file in `PUNCHLIST_CONFIG`.
- `pl runner start` registers a runner with that person's token and gets the runner's own token back, held in memory only. It needs `git`, `gh` logged in with push access to the workspace's repository, and `claude` on `PATH`. Clones and worktrees go under `--worktree-root` (default `~/.local/share/punchlist/worktrees`).
- GitHub events arrive at `POST /api/github/webhook`, verified with the GitHub App's webhook secret in `GITHUB_WEBHOOK_SECRET` (or `punchlist-server --github-webhook-secret`; it is a secret, so start the server under `with-env`). `pl issue show` lists an issue's pull requests; `pl pr unlinked` lists the ones that name no issue.
- The server reads `.punchlist/` from the workspace repository's default branch as the GitHub App, with `GITHUB_APP_ID` and `GITHUB_APP_PRIVATE_KEY` (a secret; one-line PEM is fine). The App needs Contents read and the Push event. `bootstrap` queues the first load; a push that changes `.punchlist/` queues the next. `pl workflow show` prints the active version; `GET /api/workflow/schema.json` is the JSON Schema.
- `pl issue transition <id> <status>` requests a transition and prints any failed gate; `pl issue comment <id> --body` comments. Each claim creates an agent actor whose token works only while its run runs; the runner gives it to `pl mcp`, which Claude Code gets as the `punchlist` MCP server (`get_issue`, `comment`, `request_transition`).
- Secrets never go in a `.env` file. Commands that need one run under `with-env` (the Punchlist 1Password Environment).
- `.claude/skills/verify-punchlist/` says how to run a live check.
- Live LLM tests use one model per agent and nothing else: Claude Code runs use the `haiku` alias (`--model haiku`), the newest Haiku: Haiku 5.5 on the Anthropic API, though Haiku 4.5 on Bedrock, Vertex and Foundry. Codex runs use GPT-6 Luna, `gpt-6-luna` (`codex exec -m gpt-6-luna`); OpenAI documents no alias for it, and `gpt-5.6-luna` is the older Luna, do not use it. Make a live call only inside a named live check.
- CodeRabbit does not review pull requests in this repository on its own. When a pull request is ready for review, comment `@coderabbitai review` on it to start one, and comment again after later pushes that need a fresh review.

Keep setup, scripts and layout documented here as slices land.

## Rules for the code

- The workflow lives in `.punchlist/workflow.toml`, and prompts live in `.punchlist/prompts/` (ADR 0001). Never put workflow rules in a prompt.
- A gate is a pure function in `punchlist-core` over recorded evidence. An agent's claim that a gate passed is never evidence.
- The server checks roles, locks and gates for a transition in one transaction, and records the actor, the workflow version and each gate result.
- Parse external data where it enters: workflow files, GitHub webhooks, Linear imports, agent requests. Trust typed data inside.
- Text from outside the team reaches an agent only inside a fence that the text cannot close.
- Runs are idempotent per issue and attempt. A claim is a lease, and a runner restart never loses or duplicates a run.
