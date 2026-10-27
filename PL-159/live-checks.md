# PL-159 live checks

Branch `feature/pl-159` (150f53d) on verify-punchlist instance `pl159b`; `main` (0e7f81a) on `pl159m`. Runner `r1`, `--model haiku`; every run's log reports `claude-haiku-5-5`.
Each issue's body told the agent to call `wait_for_person` with a fixed reason, then `sleep`, then stop (no sandbox prompt edit).
PL-1's reason: `<b>bold</b> *star*`. PL-2's reason: `Needs a key for the sandbox.`

| # | Check | Result | Evidence |
|---|---|---|---|
| 1 | main vs branch | PASS | main: run `succeeded`, `waiting_reason` `<b>bold</b> *star*`, `GET /api/inbox` = `{"items":[],"unread":0}`. Branch: `Run needs you` lists PL-1 (1-run-needs-you.png) |
| 2 | Reason | PASS | `[data-run-line]` = `Claude Code is waiting on a person: <b>bold</b> *star*` |
| 3 | Failed run | PASS | claude killed (-9) after the call: run `failed`, `claude was killed by a signal`; one item: `Claude Code is waiting on a person: Needs a key for the sandbox.`, `Run failed: claude was killed by a signal` |
| 4 | Run again | PASS | 1 `POST /api/issues/PL-1/retry`, URL `/w/PL/inbox`; `pl run list PL-1` shows run 58624468 after 38fb3f5f; dispatch `2 | 1 | f`; 1 `run_again` event |
| 5 | Leaves live | PASS | item gone 802 ms after the click, header count 2 → 1 at 811 ms, 0 navigations |
| 6 | Owner only | PASS | second person's `GET /api/inbox` = `{"items":[],"unread":0}` |
| 7 | Running run | PASS | 3 reads while running with `waiting_reason` set: `[]`; run ended 14:17:20.196 UTC, listed 14:17:20.286 (90 ms) |
| 8 | Text, not markup | PASS | reason line has 0 child elements, 0 `em`; the item's only `b` is the agent name `[data-agent]` the screen already had |
| 9 | Keyboard and phone | PASS | Tab order: `item PL-2`, `Run again PL-2`, `item PL-1`, `Run again PL-1`; 390 px: doc 390/390, rows 364/364 |
| 10 | Issue page agrees | PASS | notice `Waiting on a person: Claude Code is waiting on a person: Needs a key for the sandbox.`; `pl issue show PL-2`: `waiting on a person: Claude Code is waiting on a person: Needs a key for the sandbox.` |

## Rows

```
 issue_id |  outcome  |        waiting_reason        |   state   |        failure_reason         |  reported_model
 PL-1     | succeeded | <b>bold</b> *star*           | succeeded |                               | claude-haiku-5-5
 PL-2     | failed    | Needs a key for the sandbox. | failed    | claude was killed by a signal | claude-haiku-5-5
```

## Owner's inbox after both runs

```
2
{"kind":"run","id":"PL-2","last_outcome":"failed","reason":"claude was killed by a signal","waiting_reason":"Needs a key for the sandbox.","limit_reached":false}
{"kind":"run","id":"PL-1","last_outcome":"succeeded","reason":null,"waiting_reason":"<b>bold</b> *star*","limit_reached":false}
```

## Watcher (check 7, branch)

```
14:17:14.989 run 38fb3f5f-c5e1-4286-9d2e-f1bbd3e3c6f3 running, waiting_reason set; inbox items for PL-1: []
14:17:15.229 run 38fb3f5f-c5e1-4286-9d2e-f1bbd3e3c6f3 running, waiting_reason set; inbox items for PL-1: []
14:17:15.473 run 38fb3f5f-c5e1-4286-9d2e-f1bbd3e3c6f3 running, waiting_reason set; inbox items for PL-1: []
14:17:20.241 run 38fb3f5f-c5e1-4286-9d2e-f1bbd3e3c6f3 ended succeeded at 14:17:20.196 UTC
14:17:20.286 inbox items for PL-1: [{"kind":"run","last_outcome":"succeeded","reason":null,"waiting_reason":"<b>bold</b> *star*"}]
```

## Watcher (check 1, main)

```
14:19:36.157 run 31fe4b13-22ea-4f01-b7e9-44cbbfaf4037 running, waiting_reason set; inbox items for PL-1: []
14:19:36.384 run 31fe4b13-22ea-4f01-b7e9-44cbbfaf4037 running, waiting_reason set; inbox items for PL-1: []
14:19:36.610 run 31fe4b13-22ea-4f01-b7e9-44cbbfaf4037 running, waiting_reason set; inbox items for PL-1: []
14:19:41.935 run 31fe4b13-22ea-4f01-b7e9-44cbbfaf4037 ended succeeded at 14:19:41.928 UTC
14:19:46.454 not listed after 4 s
```
