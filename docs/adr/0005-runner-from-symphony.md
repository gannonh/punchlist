# 5. Copy Symphony's orchestrator into the runner crate, and keep runner state on the server

Date: 2026-10-07. Status: accepted.

## Context

ADR 0003 starts the runner from Kata Symphony's orchestrator. It left open how the code arrives and where the runner keeps its state.

Symphony is one binary crate, `symphony` v2.3.3, in `~/dev/kata-symphony/apps/symphony`: about 85,000 lines in 34 top-level modules. The parts Punchlist wants are a small share of it:

- The orchestrator loop in `orchestrator.rs` (6,891 lines): claim, run, retry with capped backoff, reconcile, detect stalls. It imports the supervisor agent, triage runtime, Docker and SSH workers, the shared-context store, notifications and the workflow store, none of which the runner needs.
- `path_safety.rs` and `workspace.rs`: a worktree per issue and its path checks.
- `codex/app_server.rs` and `pi_agent/`: the Codex and Pi adapters with token accounting.

The rest serves Symphony's own product: Linear and GitHub Projects polling, the spec, triage, implementation and review pipelines, a TUI and Liquid templates. Its `TrackerAdapter` trait reads issues by state name and writes states back, which is the two-way sync that Punchlist replaces. Symphony is not published to crates.io.

Three ways to bring the code in:

- **Depend on the crate** as a git dependency. Punchlist would compile all 85,000 lines and their dependencies, including `rusqlite`, `liquid` and `ratatui`, and would bend its runner around a trait built for polling a tracker by state name. Every Punchlist change to the loop would be a Symphony release.
- **Fork the repository.** Punchlist would carry Symphony's whole tree and history. Symphony is winding down (PRD, "What carries over"), so there is no upstream to merge from.
- **Copy the modules** the runner needs into a Punchlist crate and cut their ties to the rest of Symphony.

Symphony keeps local state in SQLite: `factory.db` for its implementation pipeline and `state.db` for its spec and triage stores. In Punchlist the server owns issues, runs, attempts and leases in Postgres (ADR 0003, PRD R8). A second store on the runner would be a second copy of run state, which is the drift the PRD names as the problem with orchestrators that sit beside a tracker.

Symphony's license is MIT, with Gannon as its only human author. ADR 0002 lists it as Apache-2.0, which is wrong. Either way, Gannon can relicense his own code under `MIT OR Apache-2.0`.

## Decision

- Copy, do not depend or fork. The runner crate `punchlist-runner` takes the orchestrator loop, `path_safety.rs`, `workspace.rs` and the Codex adapter from Symphony at commit `e7c160f5` (v2.3.3). The Pi adapter follows when R9 needs it. `path_safety.rs` is copied unchanged; the others are trimmed to what the runner uses.
- Each copied file starts with a comment naming its Symphony source path and commit, so a later reader can diff it against the original.
- The runner talks to the server through the Punchlist API. Symphony's `TrackerAdapter` and its Linear and GitHub clients are not copied.
- The runner has no database. The server's Postgres holds each run, attempt, lease and agent session id. The runner keeps only its config and token, and the worktrees on disk.
- After a restart the runner asks the server for the leases it holds. For each one it resumes the agent session when the agent supports resume and the worktree is intact. Otherwise it reports the attempt failed, and the server decides whether to retry. A lease carries an attempt number that the server checks with compare-and-set, so a stale runner cannot write to a newer attempt.

## Consequences

- The runner starts from at most about 10,700 lines of tested Symphony code (the orchestrator, path safety, workspace and Codex modules before trimming) rather than all 85,000.
- Symphony's tests for the copied modules come with them. The orchestrator tests that depend on dropped modules are rewritten or dropped.
- Fixes in Symphony after `e7c160f5` do not reach Punchlist on their own. Symphony is winding down, so few are expected.
- Run state has one owner. A runner restart cannot lose a run the server knows about, and the server never learns about a run from a runner's local file.
- The runner needs the server to recover. A runner that cannot reach the server holds its leases until they expire, and the server then fails those attempts.
