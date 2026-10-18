# Refused moves

The server refuses a move the workflow does not allow and says which rule refused it. `pl` prints `error: refused: <rule>` on stderr and exits `1`; the API answers `409` with `{"code", "message"}`. A refused move changes nothing.

## Sub-features

- `refuse-role` names the roles that may make the move: `only a runner makes start → in_progress, not a person` (code `role_not_allowed`).
- `refuse-missing` refuses a move with no transition: `no transition backlog → done exists in the workflow` (code `no_transition`).
- `refuse-event-only` refuses a move that only a GitHub event makes: `merging → done happens only on the `pr_merged` event, not on request` (code `event_only`).
- `refuse-gated` refuses a gated move, for an allowed role, when a gate fails over the recorded evidence (code `gate_failed`). `pl` and the API's `message` say `in_progress → agent_review needs gate pr_open: no open pull request is linked to PL-1` (`pl` then lists every gate's result); the agent's `request_transition` says `Refused (gate_failed): gate pr_open failed: no open pull request is linked to PL-1`. [workflow-and-gates.md](workflow-and-gates.md) covers the gates themselves.
- `refuse-locked` refuses an agent or runner acting on an issue in a status the workflow locks to them: `human_review is locked: an agent cannot act on an issue in it` (code `locked`).
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
- **Gated.** Take `PL-1` from Start to In Progress without a model: `$C curl "$RUN" -X POST -H 'content-type: application/json' -d '{"name":"r","agents":["claude-code"]}' "$SERVER_URL/api/runners"`, then `curl -s -X POST -H "Authorization: Bearer <the answer's token>" -H 'content-type: application/json' -d '{}' "$SERVER_URL/api/runs/claim"`. Within the claim's 30-second lease, `PUNCHLIST_AGENT_TOKEN=<the claim's agent_token> $C request-transition "$RUN" PL-1 agent_review`. Exit `1`, first line `Refused (gate_failed): gate pr_open failed: no open pull request is linked to PL-1`, then `Gates:` and one `  <gate>: fail (…)` line each for `pr_open`, `pr_ready` and `pr_names_issue`. `SELECT count(*) FROM transition WHERE to_status = 'agent_review'` is `0`. Keep both tokens out of evidence. An agent from `pl actor create --agent --kind claude-code` ([actors-and-proof.md](actors-and-proof.md)) gets the same refusal, and its token does not lapse.

## Gotchas

- `refuse-event-only` needs an issue in Merging, which takes an agent's gated move to Agent Review and Codex's to Human Review over a real pull request ([workflow-and-gates.md](workflow-and-gates.md)). It is covered by `punchlist-core` unit tests; report it as skipped unless the check already has an issue in Merging.
- `$C pl` passes the exit code through; check it right after the command, before another command overwrites `$?`.
