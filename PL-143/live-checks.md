# PL-143 live checks, 2026-10-24

Instances: `verify-punchlist up` under `with-env` (sandbox App credentials), `main` at 971ec01 (worktree of origin/main), branch at ba8184a. Runner `r1` from `$C runner`, with `--claude-command` pointing at a fake `claude` that reads the prompt and acts on the issue's title:

- `limit 2m …` / `limit 30m …`: prints the three Claude Code lines recorded when the limit stopped PL-60 on 2026-10-23 (`crates/punchlist-runner/tests/fixtures/claude/usage_limit.jsonl`: the `rate_limit_event` with `status: rejected`, the synthetic assistant message, the `result` with `api_error: usage_limit_reached`), with `resetsAt` set to 2 or 30 minutes after its first call for that issue, and exits 1. Once a flag file exists for the issue it runs the real `claude` with the same arguments and prompt (`--model haiku`).
- `fail …`: one stderr line, exit 1, no limit line.

## Check 1: same flow on `main` and the branch — PASS

`main` (`issue move PL-1 start` at 16:53:20):

```
$ pl run list PL-1     (16:53:50)
  274b454b…  Claude Code  started 2026-10-24 16:53:25 UTC  duration 1.4s  failed  (claude exited with code 1)
  73c0bbdb…  Claude Code  started 2026-10-24 16:53:22 UTC  duration 1.5s  failed  (claude exited with code 1)
  fe884c80…  Claude Code  started 2026-10-24 16:53:20 UTC  duration 1.4s  failed  (claude exited with code 1)
$ pl issue show PL-1
Status: In Progress
waiting on a person: 3 runs in In Progress ended without moving it
```

3 runs in 5 s, then `waiting on a person`.

Branch (`issue move PL-2 start` at 16:56:58, reset 16:59:00):

```
$ pl run list PL-2     (16:57:23)
  6fcb71a6…  Claude Code  started 2026-10-24 16:56:58 UTC  duration 1.1s  failed  tokens 0 in / 0 out  (usage limit)
    attempt 1  model haiku (pinned)  runner r1  failed  (usage limit)
$ pl issue show PL-2
Status: In Progress
waiting for the Claude Code usage limit to reset at 2026-10-24 16:59:00 UTC
$ pl runner list
r1  claude-code,codex  heartbeat 8s ago  claude-code paused until 2026-10-24 16:59:00 UTC
SELECT issue_id, run_number, run_id IS NULL AS waiting, not_before FROM dispatch WHERE issue_id='PL-2'
 PL-2 | 1 | t | 2026-10-24 16:59:00+00
SELECT r.name, p.agent, p.until FROM runner_pause p JOIN runner r ON r.id = p.runner_id
 r1 | claude-code | 2026-10-24 16:59:00+00
```

At 16:58:45, 15 s before the reset: `SELECT count(*) FROM run WHERE issue_id='PL-2'` → `1`. No second run before the reset.

The issue page shows `[data-notice="usage-limit"]`: `Usage limit: waiting for the Claude Code usage limit to reset at 2026-10-24 16:59:00 UTC.` (screenshot attached).

## Check 2: resumes — PASS

The fake was switched to the real `claude --model haiku` at 16:58:45, before the reset.

```
second run seen at 16:59:20
SELECT r.started_at, r.branch, r.outcome, a.attempt, a.state FROM run r JOIN attempt a ON a.run_id = r.id WHERE r.issue_id='PL-2'
 2026-10-24 16:56:58.723029+00 | feature/pl-2 | failed  | 1 | failed
 2026-10-24 16:59:19.281265+00 | feature/pl-2 | running | 1 | running
SELECT run_number, run_id IS NULL AS waiting, not_before FROM dispatch WHERE issue_id='PL-2'
 1 | f | 2026-10-24 16:59:00+00
```

Claimed 19 s after the reset, on the same branch, with the same dispatch (run number 1: the limited run used none). The run:

```
  e3e5aae9…  Claude Code  model claude-haiku-5-5 (pinned haiku)  runner r1  started 2026-10-24 16:59:19 UTC  duration 30.7s  succeeded  tokens 298245 in / 2447 out
```

It opened sandbox pull request #280 and moved PL-2 In Progress → Agent Review at 16:59:46.

## Check 3: other kinds — PASS

At 16:59:26 PL-3 (`limit 30m`) hit the fake limit on r1:

```
$ pl runner list
r1  claude-code,codex  heartbeat 0s ago  claude-code paused until 2026-10-24 17:29:27 UTC
```

PL-2 entered Agent Review at 16:59:46 (the Haiku run's request). With Claude Code still paused, r1 claimed its Codex dispatch:

```
SELECT r.issue_id, r.agent, r.started_at, a.state FROM run r JOIN attempt a ON a.run_id=r.id WHERE r.agent='codex'
 PL-2 | codex | 2026-10-24 16:59:50.638767+00 | running
$ pl runner list     (after the claim)
r1  claude-code,codex  heartbeat 4s ago  claude-code paused until 2026-10-24 17:29:27 UTC
```

The runner was stopped after the claim. Sandbox pull request #280 was closed and its branch deleted.

## Check 4: ordinary failure — PASS

`fail: ordinary failure`, on both:

- `main` PL-2: 3 runs, each `failed (claude exited with code 1)`, dispatch rows `1 f, 2 f, 3 f`, `waiting on a person: 3 runs in In Progress ended without moving it`.
- Branch PL-1 (`start` at 16:56:27): runs started 16:56:27, 16:56:30, 16:56:32, each `failed (claude exited with code 1)`; dispatch rows `PL-1 | 1 | f |`, `2 | f |`, `3 | f |` with no `not_before`; `waiting on a person: 3 runs in In Progress ended without moving it`; `pl runner list` showed no pause.

Redispatched at once, as on `main`.
