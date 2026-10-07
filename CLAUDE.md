# Punchlist

Punchlist is an open-source issue tracker where coding agents do the work and the tracker enforces how the work gets done. It is for solo developers and small teams that run several coding agents in parallel.

- Product spec: `docs/product/prd.md`.
- Decisions: `docs/adr/`. Read the relevant ADR before changing what it decided.
- Work: the Linear project **Punchlist** in the Kata-sh team, starting with milestone **Gate 0: Spec, stack and prototype**.

## Stack

- Backend: Rust in one Cargo workspace (ADR 0003). `punchlist-core` is pure and holds the workflow types and parser, the gates and the transition rules. The server, runner, `pl` CLI and MCP server depend on it. Postgres is the store.
- The runner starts from Kata Symphony's orchestrator in `~/dev/kata-symphony/apps/symphony`.
- Frontend: a web app in TypeScript, React and Vite, served by the Rust server, with API types generated from Rust (ADR 0004). Tailwind with shadcn/ui, as in Groundwork.
- License: MIT or Apache-2.0 (ADR 0002). Every crate sets `license = "MIT OR Apache-2.0"`.

Keep setup, scripts and layout documented here as slices land.

## Rules for the code

- The workflow lives in `.punchlist/workflow.toml`, and prompts live in `.punchlist/prompts/` (ADR 0001). Never put workflow rules in a prompt.
- A gate is a pure function in `punchlist-core` over recorded evidence. An agent's claim that a gate passed is never evidence.
- The server checks roles, locks and gates for a transition in one transaction, and records the actor, the workflow version and each gate result.
- Parse external data where it enters: workflow files, GitHub webhooks, Linear imports, agent requests. Trust typed data inside.
- Text from outside the team reaches an agent only inside a fence that the text cannot close.
- Runs are idempotent per issue and attempt. A claim is a lease, and a runner restart never loses or duplicates a run.
