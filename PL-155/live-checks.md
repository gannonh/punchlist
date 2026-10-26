# PL-155 live checks

Instance: `control-punchlist up pl155-live2` at commit e064c53 plus the pending fix (c0f7c78), headless Chromium. Setup without a model: the runner API's claim and finish (verify-punchlist `features/inbox.md`, Run items). Check 1 served `main`'s build (e9b00f8) and then the branch's build from the same server.

| Check | Result | Detail |
|---|---|---|
| 1 main | PASS | `{"item": "PL-1Run needs youIn Progress2s agoRan out AClaude Code run on your runner3 runs in In Progress ended without moving it", "runAgainButtons": 0}` |
| 1 branch | PASS | `{"item": "PL-1Run needs youIn Progress2s agoRan out AClaude Code run on your runner3 runs in In Progress ended without moving it", "runAgainButtons": 1}` |
| 2 run again | PASS | `{"runsBefore": 3, "runsAfter": 4, "claimStatus": 200, "claimedIssue": "PL-1", "claimMs": 33, "path": "/w/PL/inbox", "retries": ["/api/issues/PL-1/retry"]}` |
| 3 leaves live | PASS | `{"headerBefore": 9, "headerAfter": 8, "goneMs": 58, "headerMs": 60}` |
| 4 waiting item | PASS | `{"PL-6": {"kind": "waiting", "text": null, "runAgain": 0}}` |
| 5 not shown | PASS | `{"PL-7": {"kind": "refusal", "text": "Builder (claude-code) asked for In Progress → Agent Review. Refused: gate pr_open failed: no open pull request is linked to PL-7", "runAgain": 0}, "PL-8": {"kind": "run", "text": "Claude Code run on your runner\nRun failed: agent crashed", "runAgain": 0}, "buttonsOnPage": ["Run again PL-9", "Run again PL-5", "Run again PL-4", "Run again PL-3", "Run again PL-2", "Run again PL-1"]}` |
| 6 refused | PASS | `{"shownBefore": 1, "toastText": "PL-2 was not run again. Refused (not_waiting): PL-2 is not waiting on a person", "runsBefore": 21, "runsAfter": 21, "dispatchBefore": 4, "dispatchAfter": 4, "retries": ["/api/issues/PL-2/retry"], "itemAfter": 0}` |
| 7 button stays | PASS | `{"path": "/w/PL/inbox", "retries": ["/api/issues/PL-1/retry", "/api/issues/PL-3/retry"]}` |
| 7 title opens | PASS | `{"path": "/issues/PL-9"}` |
| 8 keyboard | PASS | `{"focused": {"tag": "BUTTON", "label": "Run again PL-4", "previous": "PL-4"}, "byRole": 1, "posted": ["/api/issues/PL-4/retry"], "path": "/w/PL/inbox"}` |
| 9 phone | PASS | `{"page": [390, 390], "item": [364, 364], "buttonInside": true, "linkBeforeButton": true}` |
| 10 pending | PASS | `{"posted": ["/api/issues/PL-5/retry"], "samples": [{"ms": 22, "text": "Running again…", "disabled": true}, {"ms": 161, "text": "Running again…", "disabled": true}, {"ms": 265, "text": "Running again…", "disabled": true}, {"ms": 369, "text": "Running again…", "disabled": true}, {"ms": 472, "text": "Running again…", "disabled": true}, {"ms": 576, "text": "Running again…", "disabled": true}, {"ms": 680, "text": "Running again…", "disabled": true}, {"ms": 783, "text": "Running again…", "disabled": true}, {"ms": 887, "text": "Running again…", "disabled": true}, {"ms": 990, "text": "Running again…", "disabled": true}, {"ms": 1192, "gone": true}]}` |

Check 6: the built-in workflow has no person move out of In Progress, so another tab ran PL-2 again from its issue page while this tab's event stream was held (`page.route` aborting `/events`), leaving it stale. Its click answered 409 `not_waiting`; the item then left through `useRunAgain`'s inbox read, since the stream was held.

Check 2 rows (`SELECT ... FROM run JOIN attempt ... WHERE r.issue_id = 'PL-1'`):

```
issue_id |  outcome  |   state   | run_number | runs_before 
----------+-----------+-----------+------------+-------------
 PL-1     | succeeded | succeeded |          1 |           0
 PL-1     | succeeded | succeeded |          2 |           0
 PL-1     | succeeded | succeeded |          3 |           0
 PL-1     | running   | running   |          4 |           3
(4 rows)
```

Seeded inbox (`GET /api/workspaces/{id}/inbox`: kind, id, limit_reached, last_outcome):

```
["refusal","PL-7",null,null]
["run","PL-8",false,"failed"]
["run","PL-9",true,"succeeded"]
["waiting","PL-6",null,null]
["run","PL-5",true,"succeeded"]
["run","PL-4",true,"succeeded"]
["run","PL-3",true,"succeeded"]
["run","PL-2",true,"succeeded"]
["run","PL-1",true,"succeeded"]
```
