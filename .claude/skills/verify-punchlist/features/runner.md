# Runners, claims and runs

`pl runner start` registers a runner with the server, sends a heartbeat every 10 seconds, and long-polls `POST /api/runs/claim`. When a person moves an issue to Start, the server moves it to In Progress as the runner's actor, creates a run and its first attempt under a 30-second lease, and returns the claim. The runner adds a worktree on `feature/<id>-<slug>` from the repository's default branch, starts `claude -p` with the issue inside a fence, streams the agent's output to the run log, and reports the outcome with its duration and tokens. `pl issue show` lists the issue's runs with the last 20 log lines; `pl run log` prints the whole log.

## Sub-features

- `register` makes a runner actor (role `runner`) and a `runner` row; `pl runner list` shows it with its last heartbeat.
- `heartbeat` updates `runner.last_heartbeat` and renews only the leases of the attempts the body lists in `held`; a claim the runner never received expires.
- `claim` moves `start → in_progress` as the runner, writes a `run` row and an `attempt` row with `attempt = 1`.
- `claim-taken` is what a second runner gets when it raced for the same issue: its log says `claim taken: PL-<n> was claimed by runner <name>`, and no second `attempt` row exists.
- `worktree` is a checkout under the runner's `--worktree-root` on the issue's branch.
- `log` streams the agent's stdout (stream-json lines) and stderr (`stderr: ` prefix) to `run_log`. Each line carries its per-attempt `line_no`, so a resent batch is stored once.
- `finish` records `succeeded` or `failed`, `duration_ms`, `input_tokens` and `output_tokens`.
- `lease-expiry` fails an attempt whose runner stopped heartbeating with `lease expired: runner <name> stopped sending heartbeats`.

## How to get to it (user POV)

- `pl runner start [--name N] [--worktree-root DIR] [--model M] [--max-concurrent K]`, `pl runner list`.
- `pl issue move <id> start`, then `pl issue show <id>` and `pl run log <run-id>`.
- `POST /api/runners`, `POST /api/runners/{id}/heartbeat`, `POST /api/runs/claim`, `POST /api/runs/{id}/log`, `POST /api/runs/{id}/finish`, `GET /api/issues/{id}/runs`, `GET /api/runs/{id}/log`.

## Driving it with control-punchlist

Preconditions:

- Baseline run `$RUN`. `up` bootstraps the workspace with repository `gannonh/punchlist-sandbox`, so runners clone and push only there.
- `gh auth status` shows a logged-in account that can push to the sandbox, and `git ls-remote https://github.com/gannonh/punchlist-sandbox.git HEAD` answers.
- An agent run makes a live model call. Start a runner that will claim an issue only inside a named live check. `$C runner` passes `--model haiku`, the alias for the newest Haiku (Haiku 5.5 on the Anthropic API). Codex runs use GPT-6 Luna (`gpt-6-luna`), once the runner has a Codex adapter.

- **Start a runner.** `$C runner "$RUN" r1`. It waits until the log at `$EVIDENCE/runner-r1.log` says the runner registered.
- **List.** `$C pl "$RUN" runner list`. One line naming `r1`, `claude-code` and a heartbeat a few seconds old. `$C sql "$RUN" "SELECT id, name, last_heartbeat FROM runner"` has one row.
- **Claim.** `$C pl "$RUN" issue create --title "Add a hello file" --body "Create hello.txt containing hello."`, `$C pl "$RUN" issue move PL-1 todo`, `$C pl "$RUN" issue move PL-1 start`, wait 10 s, `$C pl "$RUN" issue show PL-1`. `Status: In Progress`, a timeline line `r1 (runner)  Start → In Progress`, and a run with agent `Claude Code`, runner `r1`, outcome `running`. `$C sql "$RUN" "SELECT issue_id, runner_id, attempt, state FROM attempt"` has one row.
- **Worktree.** `git -C /tmp/punchlist-verify/$RUN/worktrees/r1/worktrees/gannonh/punchlist-sandbox/PL-1 branch --show-current` prints `feature/pl-1-add-a-hello-file`.
- **Log.** `$C pl "$RUN" run log <run-id>` while the agent works, and again after. The line count grows. After the run, a line contains `gh pr create`.
- **Finish.** `$C sql "$RUN" "SELECT state, duration_ms, input_tokens, output_tokens FROM attempt"` shows `succeeded` and non-null numbers. `gh pr list --repo gannonh/punchlist-sandbox --draft --search "PL-1 in:title"` lists the agent's pull request.
- **Contention.** `$C runner "$RUN" r1` and `$C runner "$RUN" r2`, then move one issue to Start. `$C sql "$RUN" "SELECT count(*) FROM attempt WHERE issue_id = 'PL-<n>'"` is `1`, and the losing runner's log has `claim taken: PL-<n> was claimed by runner <winner>`.
- **Stop.** `$C runner-stop "$RUN" r1` sends SIGINT; a running attempt finishes `failed` with reason `runner stopped`. `down` stops every runner in the run.

## Gotchas

- Each run's runners clone the sandbox under `/tmp/punchlist-verify/<RUN_ID>/worktrees/<name>/`, which `down` deletes. Branches and pull requests the agent pushed stay in the sandbox: close the pull request and delete the branch after the check (`gh pr close <n> --repo gannonh/punchlist-sandbox --delete-branch`).
- Sandbox branches are named after the issue id, and every run numbers issues from `PL-1`. Before a run, check `git ls-remote --heads https://github.com/gannonh/punchlist-sandbox.git 'feature/pl-*'` and delete leftovers, or the agent's push collides.
- Contention is deterministic only when both runners are already long-polling when the issue enters Start: start both, wait for both to register, then move the issue.
- A runner holds its token in memory only. Restarting it registers a new runner; the old one's leases expire after 30 s and their attempts fail.
