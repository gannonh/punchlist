# Refused moves

The server refuses a move the workflow does not allow and says which rule refused it. `pl` prints `error: refused: <rule>` on stderr and exits `1`; the API answers `409` with `{"code", "message"}`. A refused move changes nothing.

## Sub-features

- `refuse-role` names the roles that may make the move: `only a runner makes start → in_progress, not a person` (code `role_not_allowed`).
- `refuse-missing` refuses a move with no transition: `no transition backlog → done exists in the workflow` (code `no_transition`).
- `refuse-event-only` refuses a move that only a GitHub event makes: `merging → done happens only on the `pr_merged` event, not on request` (code `event_only`).
- `refuse-gated` refuses a move whose gate fails, naming the first failed gate and its reason: `in_progress → agent_review needs gate pr_ready: pull request #<n> is a draft` (code `gate_failed`). The 409 body also carries `gate`, `reason`, `evidence` and every gate's result. Only an agent can make a gated move; see [workflow-and-gates.md](workflow-and-gates.md).
- `refuse-unknown` refuses a status not in the workflow: ``status `shipped` is not in the workflow`` (code `unknown_status`).
- `refuse-no-write` leaves the status, `transition` and `event` tables unchanged.

## How to get to it (user POV)

- `pl issue move <id> <status>` with a move the workflow does not allow.
- `POST /api/issues/{id}/transitions`.

## Driving it with control-punchlist

Preconditions:

- Baseline run `$RUN` with `PL-1` moved to Start (see [issue-move.md](issue-move.md)) and `PL-2` created in Backlog.

- **Count before.** Run `$C sql "$RUN" "SELECT count(*) FROM transition"`. Note the count (2 after the move recipe).
- **Role rule.** Run `$C pl "$RUN" issue move PL-1 in_progress`; then `echo $?`. Exit `1`, stdout empty, stderr exactly `error: refused: only a runner makes start → in_progress, not a person`.
- **Missing transition.** Run `$C pl "$RUN" issue move PL-2 done`. Exit `1`, stderr exactly `error: refused: no transition backlog → done exists in the workflow`.
- **Unknown status.** Run `$C pl "$RUN" issue move PL-2 shipped`. Exit `1`, stderr ``error: refused: status `shipped` is not in the workflow``.
- **Nothing written.** Run `$C sql "$RUN" "SELECT count(*) FROM transition"` and `$C sql "$RUN" "SELECT id, status FROM issue ORDER BY id"`. The count is unchanged; `PL-1 | start`, `PL-2 | backlog`.

## Gotchas

- `refuse-gated` needs an agent's token, which only a run's `pl mcp` holds. Drive it with the runner recipe in [workflow-and-gates.md](workflow-and-gates.md); a person asking for the same move gets `refuse-role` or `refuse-missing` before any gate runs.

- `refuse-event-only` cannot be reached yet: the issue would have to be in Merging, which follows Human Review, and `agent_review → human_review` needs gates Punchlist does not evaluate yet (`ci_green`, `mergeable`, `no_open_threads`), so it fails closed. It is covered by `punchlist-core` unit tests; report it as skipped in live checks.
- `$C pl` passes the exit code through; check it right after the command, before another command overwrites `$?`.
