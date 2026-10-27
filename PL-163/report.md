# PL-163 live checks

Branch `feature/pl-163` (431ff4e) on run `pl163b`; `main` (fe5deb6) on run `pl163m`. Sandbox `gannonh/punchlist-sandbox`, Claude Code on `haiku` (runs report `claude-haiku-5-5`).

PL-1's body tells the agent to call `wait_for_person` with the reason `<b>x</b>`. Haiku did, in each of its 4 runs (`run.waiting_reason` = `<b>x</b>`). `main`'s PL-1 got the same reason through the API: a runner claim, the agent token's `POST /api/runs/{id}/waiting`, then finish.

| # | Check | Result | Observed |
|---|---|---|---|
| 1 | Same flow on main and the branch | PASS | main: sheet has 0 `[data-notice=waiting]`, cell `Idle`. branch: 1 notice, `Waiting on a person: Claude Code is waiting on a person: <b>x</b>.`, cell `Waiting on you` |
| 2 | Row | PASS | PL-1 cell `Waiting on you`, title `Claude Code is waiting on a person: <b>x</b>`; PL-2 (Backlog) `Idle` |
| 3 | Run again from the sheet | PASS | 1 `POST /api/issues/PL-1/retry`; notice gone in 424 ms, cell `Running · 0s` in 428 ms; 0 reloads; `pl run list PL-1` shows the new run |
| 4 | Board | PASS | card `[data-run]` reads `Waiting on you` |
| 5 | Agent view (bearer header) | PASS | `data-viewer=agent`, notice shows `<b>x</b>`, 0 `[data-run-again]` |
| 6 | Phone notice, 390 px | PASS | header bottom 153, notice top 167, Status box top 238 |
| 7 | Phone sheet, 390 px | PASS | sheet scrollTop 0; notice 79–136, button 95–119, viewport 844 |
| 8 | Text, not markup | PASS | sheet, page and row title all contain the literal `<b>x</b>`; 0 `b` elements in either notice |
| 9 | Keyboard | PASS | 1 Tab reaches `Run again PL-1`; Enter sends 1 retry POST; 0 reloads |
| 10 | Not waiting | PASS | PL-4 (running run): cell `Running · 1s`, 0 notices on the sheet and the page; PL-2 (Backlog): `Idle`, 0 notices |

Check 10 first ran on PL-3 and passed. Restarting the Haiku runner for the video let it pick up PL-3's lapsed attempt (its API runner had stopped heartbeating) and run it 3 times, so PL-3 really did wait (`3 runs in In Progress ended without moving it`), and the row and sheet showed that. The final rerun uses PL-4, with no runner able to take its attempt.

Rows behind checks 3 and 9:

```
 issue_id | run_number | runs_before |  outcome  | waiting_reason
 PL-1     |          1 |           0 | succeeded | <b>x</b>
 PL-1     |          2 |           1 | succeeded | <b>x</b>
 PL-1     |          3 |           2 | succeeded | <b>x</b>
 PL-1     |          4 |           3 | succeeded | <b>x</b>

 seq |   kind    |  role
   8 | comment   | agent
  10 | run_again | person     <- check 3 (click)
  13 | comment   | agent
  15 | run_again | person     <- check 9 (Tab, Enter)
  18 | comment   | agent
  31 | run_again | person     <- the video
  36 | comment   | agent
```

Tests: `pnpm check` passes. `pnpm e2e` passes (25 tests). The server test is `a_waiting_issues_row_says_why_and_anothers_says_nothing`, the unit test is in `issues.test.ts`, and the e2e test is `the issue page, the list row and the side sheet show an issue waiting on a person; Run again from the sheet`.
