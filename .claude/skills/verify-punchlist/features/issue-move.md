# Move an issue and read its timeline

A person moves an issue along the workflow with `pl issue move`. The server checks who may make the move, then updates the status and writes a `transition` row and an `event` row in one transaction, recording the actor and the workflow version. `pl issue show` lists each move in the timeline with the actor's name and role.

## Sub-features

- `move-ok` prints `<id>  <From> → <To>` and changes the status.
- `move-records` writes one `transition` row and one `event` row per move, both naming the actor.
- `move-version` records the workflow's content hash (`sha256:...`) on the transition.
- `timeline` shows each move, oldest first, as `<time>  <actor> (<role>)  <From> → <To>`.

## How to get to it (user POV)

- `pl issue move <id> <status>`, then `pl issue show <id>`.
- `POST /api/issues/{id}/transitions` with `{"to": "<status>"}`; `GET /api/issues/{id}/events`.

## Driving it with control-punchlist

Preconditions:

- Baseline run `$RUN` with `PL-1` created and in Backlog (see [issue-create-show.md](issue-create-show.md)).

- **To Todo.** Run `$C pl "$RUN" issue move PL-1 todo`. Exit `0`, stdout `PL-1  Backlog → Todo`.
- **To Start.** Run `$C pl "$RUN" issue move PL-1 start`. Exit `0`, stdout `PL-1  Todo → Start`.
- **Timeline.** Run `$C pl "$RUN" issue show PL-1`. Stdout has `Status: Start` and exactly two timeline lines, `Verify Person (person)  Backlog → Todo` then `Verify Person (person)  Todo → Start`, each after a UTC time.
- **Rows.** Run `$C sql "$RUN" "SELECT from_status, to_status, actor_id FROM transition ORDER BY created_at"` and `$C sql "$RUN" "SELECT id, name, role FROM actor"`. Two rows, `backlog | todo` and `todo | start`, both with the actor id of `Verify Person`.
- **Events.** Run `$C sql "$RUN" "SELECT seq, issue_id, kind, actor_id FROM event ORDER BY seq"`. Rows `1 | PL-1 | transition` and `2 | PL-1 | transition`, same actor id.
- **Version.** Run `$C sql "$RUN" "SELECT DISTINCT workflow_version FROM transition"`. One row starting `sha256:`.

## Gotchas

- A person can only make `backlog → todo`, `todo → start` and `human_review → merging`. `start → in_progress` needs a runner; see [issue-refusals.md](issue-refusals.md).
- Event `seq` counts per workspace across all issues, not per issue.
- Timeline times are UTC; compare order, not wall-clock values.
