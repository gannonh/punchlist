# PL-111 live checks, run pl111-final (headless Chromium, branch build at 6f47640)

Workflow: the sandbox's, plus for check 3 one transition `backlog → canceled` by person with `gates = ["proof_attached"]`, pushed to the sandbox's `main` at 8a95938 and restored at 23707ba (sha256 4fa930e9… matches the original).

1. **main vs branch** PASS. main's menu items: ['AssigneeA', 'LabelsL', 'Project⇧P', 'Milestone⇧M', 'Copy ID']; branch's: ['StatusS', 'AssigneeA', 'LabelsL', 'Project⇧P', 'Milestone⇧M', 'Copy ID'], and it moved PL-1, PL-2:
```
id  | status 
------+--------
 PL-1 | todo
 PL-2 | todo
(2 rows)
```
2. **All move** PASS. `SELECT count(*) FROM transition WHERE to_status = 'todo'` before 2, after 5. Rows with each one's event:
```
issue_id | from_status | to_status | seq 
----------+-------------+-----------+-----
 PL-3     | backlog     | todo      |   8
 PL-4     | backlog     | todo      |   9
 PL-5     | backlog     | todo      |  10
(3 rows)
```
3. **Partial** PASS. Toast `Moved 2 to Canceled; PL-7 refused: gate proof_attached failed`. PL-7's row: `Canceled: Refused (gate_failed): gate proof_attached failed: no proof is attached to PL-7 for attempt 0`. Canceled transitions 0 → 2; PL-7 has 0 transitions.
```
id  |  status  
------+----------
 PL-6 | canceled
 PL-7 | backlog
 PL-8 | canceled
(3 rows)
```
4. **Role** PASS (as a person, per the edited spec). With PL-9, PL-10 in Start: In Progress `aria-disabled=true`, title `only a runner makes start → in_progress, not a person`.
5. **Pending** PASS. With each answer held 1.5 s in the browser, all three rows read ['Moving to Todo…', 'Moving to Todo…', 'Moving to Todo…'] while still ['backlog', 'backlog', 'backlog']; the toast came 1869 ms after the click.
6. **Keys** PASS. `s` with three rows checked opened the menu `Status of 3 issues`.
7. **Live** PASS. A second browser showed all three rows in Todo 315 ms after the last answer.
8. **Clear** PASS. `3 selectedClear` before, no `[data-selection]` after the full move; `3 selectedClear` after the partial one.
9. **Phone** PASS. 390×844 touch: a long-press opened `Change 2 issues`; the Status submenu box {'x': 106, 'y': 461, 'width': 256, 'height': 288.6875} is inside the viewport; tapping Todo moved both:
```
id   | status 
-------+--------
 PL-11 | todo
 PL-12 | todo
(2 rows)
```
10. **Render** PASS. Per moved row (commit duration ÷ rows that moved into Todo in it): median single move 0.6 ms (5 moves: [0.8, 0.6, 0.6, 0.6, 0.6]), median in the 50-issue move 0.6 ms; 50 issues moved. Timer resolution is 0.1 ms. Before the fix in 6f47640 the 50-issue move's first ~20 rows cost 2 to 2.5 ms each: the context menu's Status picker re-rendered on each answer while it faded out.

Disabled-state picker for three Backlog issues:
```
[
 {
  "name": "Backlog",
  "checked": "true",
  "disabled": "true",
  "title": "already in Backlog"
 },
 {
  "name": "Todo",
  "checked": "false",
  "disabled": null,
  "title": null
 },
 {
  "name": "Start",
  "checked": "false",
  "disabled": "true",
  "title": "Backlog → Start cannot be requested"
 },
 {
  "name": "In Progress",
  "checked": "false",
  "disabled": "true",
  "title": "Backlog → In Progress cannot be requested"
 },
 {
  "name": "Agent Review",
  "checked": "false",
  "disabled": "true",
  "title": "Backlog → Agent Review cannot be requested"
 },
 {
  "name": "Human Review people only",
  "checked": "false",
  "disabled": "true",
  "title": "Backlog → Human Review cannot be requested"
 },
 {
  "name": "Merging",
  "checked": "false",
  "disabled": "true",
  "title": "Backlog → Merging cannot be requested"
 },
 {
  "name": "Done",
  "checked": "false",
  "disabled": "true",
  "title": "Backlog → Done cannot be requested"
 },
 {
  "name": "Canceled",
  "checked": "false",
  "disabled": null,
  "title": null
 }
]
```
