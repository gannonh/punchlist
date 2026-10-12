# Runners, claims and runs

`pl runner start` registers a runner with the server, sends a heartbeat every 10 seconds, and long-polls `POST /api/runs/claim`. When a person moves an issue to Start, the server moves it to In Progress as the runner's actor, creates a run and its first attempt under a 30-second lease, and returns the claim. The runner adds a worktree on `feature/<id>-<slug>` from the repository's default branch, starts `claude -p` with the issue inside a fence, streams the agent's output to the run log, and reports the outcome with its duration and tokens. `pl issue show` lists the issue's runs with the last 20 log lines; `pl run log` prints the whole log. When a transition puts an issue in a status with a `dispatch.status` rule (Agent Review, `agent = "codex"`), the server writes a `dispatch` row, and a runner registered with that agent claims it as soon as the issue's running run has ended: no transition, a second run on the same branch and in the same worktree, running `codex app-server` with the status's prompt.

## Sub-features

- `register` makes a runner actor (role `runner`) and a `runner` row; `pl runner list` shows it with its last heartbeat.
- `heartbeat` updates `runner.last_heartbeat` and renews only the leases of the attempts the body lists in `held`; a claim the runner never received expires.
- `claim` moves `start → in_progress` as the runner, writes a `run` row and an `attempt` row with `attempt = 1`.
- `claim-taken` is what a second runner gets when it raced for the same issue: its log says `claim taken: PL-<n> was claimed by runner <name>`, and no second `attempt` row exists.
- `worktree` is a checkout under the runner's `--worktree-root` on the issue's branch.
- `log` streams the agent's stdout (stream-json lines) and stderr (`stderr: ` prefix) to `run_log`. Each line carries its per-attempt `line_no`, so a resent batch is stored once.
- `finish` records `succeeded` or `failed`, `duration_ms`, `input_tokens` and `output_tokens`.
- `lease-expiry` fails an attempt whose runner stopped heartbeating with `lease expired: runner <name> stopped sending heartbeats`.
- `dispatch-on-entry` writes one `dispatch` row per transition into a status with a dispatch rule (`agent`, and the `transition_id` whose `to_status` it is for); the issue's next transition deletes one still unclaimed, and the claim sets its `run_id`. The claim from Start writes and claims its own row in one transaction.
- `dispatch-again` queues one more `dispatch` row when a run ends (`finish` with either outcome, or `lease-expiry`) and its issue has made no transition since the one that dispatched the run: same `transition_id` and `agent`, `run_number` one higher, written in the transaction that ends the run. It applies to every dispatch, so to the Claude Code run in In Progress too. A run that moved its issue, or whose issue a person or a GitHub event moved, queues nothing. The limit is 3 runs per transition (`RUNS_PER_ENTRY` in `runs.rs`); a new transition into the status starts at `run_number` 1.
- `waiting-on-person`: after the third run of one entry ends with the issue still there, no row is queued, and `pl issue show` prints `waiting on a person: 3 runs in <status name> ended without moving it` on the line under `Status:` (`waiting_on_person` in `GET /api/issues/{id}`; absent otherwise).
- `codex-run` is the run a runner started with `--agents claude-code,codex` gets for that row: `pl issue show` lists it above the Claude Code run as `Codex  runner <name>`, on the same branch, with token counts from Codex's `thread/tokenUsage/updated`. Its log is the app-server's JSON-RPC messages, without the streaming deltas.

## How to get to it (user POV)

- `pl runner start [--name N] [--worktree-root DIR] [--agents claude-code,codex] [--model M] [--codex-model M] [--codex-command PATH] [--max-concurrent K]`, `pl runner list`.
- `pl issue move <id> start`, then `pl issue show <id>` and `pl run log <run-id>`.
- `POST /api/runners`, `POST /api/runners/{id}/heartbeat`, `POST /api/runs/claim`, `POST /api/runs/{id}/log`, `POST /api/runs/{id}/finish`, `GET /api/issues/{id}/runs`, `GET /api/runs/{id}/log`.

## Driving it with control-punchlist

Preconditions:

- Baseline run `$RUN`. `up` bootstraps the workspace with repository `gannonh/punchlist-sandbox`, so runners clone and push only there.
- `gh auth status` shows a logged-in account that can push to the sandbox, and `git ls-remote https://github.com/gannonh/punchlist-sandbox.git HEAD` answers.
- An agent run makes a live model call. Start a runner that will claim an issue only inside a named live check. `$C runner` passes `--model haiku`, the alias for the newest Haiku (Haiku 5.5 on the Anthropic API), and `--codex-model gpt-6-luna` for Codex runs. A runner claims Codex dispatches only when started with `--agents claude-code,codex`, and then needs `codex` on `PATH`, logged in (`codex login status`).

- **Start a runner.** `$C runner "$RUN" r1`. It waits until the log at `$EVIDENCE/runner-r1.log` says the runner registered.
- **List.** `$C pl "$RUN" runner list`. One line naming `r1`, `claude-code` and a heartbeat a few seconds old. `$C sql "$RUN" "SELECT id, name, last_heartbeat FROM runner"` has one row.
- **Claim.** `$C pl "$RUN" issue create --title "Add a hello file" --body "Create hello.txt containing hello."`, `$C pl "$RUN" issue move PL-1 todo`, `$C pl "$RUN" issue move PL-1 start`, wait 10 s, `$C pl "$RUN" issue show PL-1`. `Status: In Progress`, a timeline line `r1 (runner)  Start → In Progress`, and a run with agent `Claude Code`, runner `r1`, outcome `running`. `$C sql "$RUN" "SELECT issue_id, runner_id, attempt, state FROM attempt"` has one row.
- **Worktree.** `git -C /tmp/punchlist-verify/$RUN/worktrees/r1/worktrees/gannonh/punchlist-sandbox/PL-1 branch --show-current` prints `feature/pl-1-add-a-hello-file`.
- **Log.** `$C pl "$RUN" run log <run-id>` while the agent works, and again after. The line count grows. After the run, a line contains `gh pr create`.
- **Finish.** `$C sql "$RUN" "SELECT state, duration_ms, input_tokens, output_tokens FROM attempt"` shows `succeeded` and non-null numbers. `gh pr list --repo gannonh/punchlist-sandbox --draft --search "PL-1 in:title"` lists the agent's pull request.
- **Contention.** `$C runner "$RUN" r1` and `$C runner "$RUN" r2`, then move one issue to Start. `$C sql "$RUN" "SELECT count(*) FROM attempt WHERE issue_id = 'PL-<n>'"` is `1`, and the losing runner's log has `claim taken: PL-<n> was claimed by runner <winner>`.
- **Codex on entry.** `$C runner "$RUN" r1 --agents claude-code,codex`, then the **Claim** steps with a body Claude Code can finish. When its run requests Agent Review, `$C pl "$RUN" issue show PL-1` lists a second run, `Codex  runner r1`, within seconds of the first one ending; `$C sql "$RUN" "SELECT r.agent, r.status, r.branch FROM attempt a JOIN run r ON r.id = a.run_id WHERE a.issue_id = 'PL-1' ORDER BY a.started_at"` shows `claude-code | in_progress` then `codex | agent_review` on one branch, and `SELECT t.to_status, d.agent, d.run_id IS NOT NULL FROM dispatch d JOIN transition t ON t.id = d.transition_id ORDER BY d.created_at` shows both rows claimed. With a passing check and no open thread the Codex run moves the issue to Human Review. A runner without `codex` in `--agents` leaves the `dispatch` row unclaimed (`run_id` NULL), as `main` before this slice left the issue: no run starts.
- **Stop.** `$C runner-stop "$RUN" r1` sends SIGINT; a running attempt finishes `failed` with reason `runner stopped`. `down` stops every runner in the run.

## Gotchas

- Each run's runners clone the sandbox under `/tmp/punchlist-verify/<RUN_ID>/worktrees/<name>/`, which `down` deletes. Branches and pull requests the agent pushed stay in the sandbox: close the pull request and delete the branch after the check (`gh pr close <n> --repo gannonh/punchlist-sandbox --delete-branch`).
- Sandbox branches are named after the issue id, and every run numbers issues from `PL-1`. Before a run, check `git ls-remote --heads https://github.com/gannonh/punchlist-sandbox.git 'feature/pl-*'` and delete leftovers, or the agent's push collides.
- Contention is deterministic only when both runners are already long-polling when the issue enters Start: start both, wait for both to register, then move the issue.
- A claim for an issue that ran before gets the same branch name while no pull request on it has merged: the runner reuses the issue's worktree, or without one starts from the branch on the remote.
- Once the branch's pull request has merged and none is open on it, such as after a person moved the issue `merging → todo` following a refused merge, the claim names `feature/<id>-<slug>-2` (then `-3`) and lists the issue's merged branches in `merged_branches`. The runner starts that branch from the default branch and replaces a clean worktree left on a merged branch; one with uncommitted or untracked files fails the run with `has uncommitted work on the merged branch`. The run opens a second pull request that lists only its own changes; `pl issue show` lists both, and the gates read the open one.
- The claim reads the recorded pull requests: a claim made before the merge's webhook arrives continues on the merged branch.
- A dispatch waits while the issue has a running run: the Claude Code run that requested Agent Review still has to exit. The claim follows its `finish` by milliseconds; measure from the run's `finished_at`, not only from the transition.
- Codex loads the person's `~/.codex/config.toml` too: its reasoning effort and its other MCP servers come along. Only the model, the sandbox (`danger-full-access`, no approvals) and Punchlist's MCP server are set by the runner.
- A runner that still has the issue's worktree fast-forwards it to the branch on the remote before the agent starts, so a push by another runner or a person is there; a worktree that diverged from the remote fails the run with `cannot fast-forward to origin/<branch>`, and so does a remote that cannot be asked for the branch (`git ls-remote: …`); a branch the remote does not have yet is fine.
- A Codex thread is started `ephemeral`: Codex keeps no rollout of it under `~/.codex/sessions`.
- A runner holds its token in memory only. Restarting it registers a new runner; the old one's leases expire after 30 s and their attempts fail. Each such run's issue is then dispatched again (`dispatch-again`), so the new runner claims it at its next poll.
- Any run that ends without moving its issue is followed by another, up to three per entry: `runner-stop` in the middle of a run, an agent that gives up, or an issue body that never asks for a pull request each cost up to three model runs while a runner that can run the agent is up. To stop that, stop the runner or move the issue (`pl issue move PL-<n> todo`).
- A lease that expires queues the next dispatch without waking long-polling runners; it is claimed at the runner's next poll, up to 30 s later. A `finish` wakes them at once.
