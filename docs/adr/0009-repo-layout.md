# 9. Keep the Cargo workspace and the web app in one repository, driven by pnpm scripts

Date: 2026-10-07. Status: proposed.

## Context

ADR 0003 puts the backend in one Cargo workspace, and ADR 0004 adds a web app that the Rust server serves, with types generated from Rust (ADR 0008). A change to an API type touches both, so both belong in one repository and one pull request.

ADR 0003 also asks for small crates so `cargo check` stays fast in agents' edit-and-check loops. The binaries are the server and `pl`; the PRD starts the runner with `pl runner start`, and agents attach the MCP server through `pl`.

Each project needs one entry point for its commands. Options were `just`, `cargo xtask` or pnpm scripts at the root. The web app already needs pnpm, and Groundwork drives everything through pnpm scripts. `just` would be one more tool to install, and `cargo xtask` would put web commands behind Cargo.

## Decision

```text
Cargo.toml              # workspace: members, shared dependencies, lints
rust-toolchain.toml     # pinned stable Rust, with rustfmt, clippy and the wasm32 target
package.json            # root scripts; pnpm-workspace.yaml lists web/
.sqlx/                  # sqlx offline query data (ADR 0007)
crates/
  punchlist-core/       # pure: workflow types and parser, gates, transition rules; builds for wasm32
  punchlist-api/        # API request and response types (ADR 0008)
  punchlist-server/     # axum server, Postgres, migrations/, jobs; binary punchlist-server
  punchlist-client/     # Rust client for the API, used by the runner, MCP server and CLI
  punchlist-runner/     # runner, from Symphony's orchestrator (ADR 0005)
  punchlist-mcp/        # MCP server over stdio
  punchlist-cli/        # binary pl
web/                    # React, Vite, TanStack Router and Query, Tailwind, shadcn/ui
  src/api/              # openapi.json and schema.d.ts, generated
.punchlist/             # Punchlist's own workflow and prompts, for dogfooding
docker-compose.yml      # Postgres for development; the self-host stack (R20)
```

- A crate is created by the first slice that needs it. The tree above is the target, not a scaffold to build ahead of time.
- Root commands:
  - `pnpm dev` starts Postgres in Docker, the server with `cargo run -p punchlist-server`, and Vite's dev server proxying `/api` to it.
  - `pnpm check` runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, the web app's typecheck, lint and unit tests, and the generated-file checks from ADRs 0007 and 0008.
  - `pnpm gen:api` regenerates the OpenAPI document and the TypeScript types.
  - `pnpm build` builds `web/dist` and then the release binaries.
  - `pnpm e2e` runs Playwright against a built server.
- The server serves `web/dist` from a directory set in its config. The Docker image copies it next to the binary.
- CI runs `pnpm check` and `pnpm build`.

## Consequences

- One pull request carries an API change, its generated types and the screen that uses them.
- A developer needs Rust (through rustup, which reads `rust-toolchain.toml`), Node, pnpm and Docker. sartre has Node, pnpm, Docker and Postgres today but no Rust toolchain, so the first slice installs rustup there.
- Agents use the same commands in every worktree: `pnpm check` before a pull request, `pnpm dev` to drive the app.
- Seven crates add `Cargo.toml` files to maintain. Each has one job, and `cargo check -p <crate>` checks only what an edit touched.
