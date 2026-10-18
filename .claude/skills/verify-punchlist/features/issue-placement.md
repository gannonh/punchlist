# Projects, milestones and an issue's parent

A person creates a workspace's projects and their milestones, each milestone with the "PASS when" text that says it is done, and places an issue under a parent issue, in a project and in one of that project's milestones. `pl issue edit` and the web app's issue sheet set and clear the three fields, and the server refuses a parent cycle, anything outside the workspace and a milestone outside the issue's project.

## Sub-features

- `project-create`: `pl project create <name>` prints `Created project <name>.`; a name the workspace has is refused: `error: a project named <name> already exists (422 Unprocessable Entity)`.
- `milestone-create`: `pl milestone create <name> --project <project> [--pass-when <text>]` prints `Created milestone <name> in <project>.`
- `project-list`: `pl project list` prints each project, oldest first, and under it each milestone with `PASS when <text>` when it has one, or `No milestones.`; `No projects.` when there are none.
- `issue-edit`: `pl issue edit <id>` with `--parent <id>`, `--project <name>`, `--milestone <name>` or `--no-parent`, `--no-project`, `--no-milestone` prints `PL-2  parent PL-1 · project Punchlist · milestone Gate 0` (`none` for an unset one). A milestone is looked up in `--project`, else in the issue's project. With no flag it exits 2.
- `issue-show`: `pl issue show` prints `Parent: <id>`, `Project: <name>` and `Milestone: <name>` under `Status:` for the ones that are set, and the timeline entry `edited parent, project, milestone`.
- `refusals` (422 `invalid`, nothing written): `PL-1 cannot be its own parent`, `PL-3 is under PL-1 (PL-1 → PL-2 → PL-3), so it cannot be PL-1's parent`, `no issue OT-1 in this workspace`, `no project <id> in this workspace`, `no milestone <id> in this workspace`, `milestone Gate 0 is in project Punchlist; give PL-2 that project to set it`.
- `project-change`: a new project clears the milestone unless the same request sets one; the `edited` event names `project` and `milestone`.

## How to get to it (user POV)

- `pl project create`, `pl project list`, `pl milestone create`, `pl issue edit`, `pl issue show`.
- `GET /api/projects`, `POST /api/projects` `{"name"}`, `POST /api/projects/{id}/milestones` `{"name", "pass_when"}`, and `PATCH /api/issues/{id}` with `parent` (an issue id), `project` and `milestone` (ids), `null` clearing each.
- The issue sheet in the web app: [web-app.md](web-app.md), `web-edit`.

## Driving it with control-punchlist

Preconditions:

- Baseline run `$RUN` with three issues: `$C pl "$RUN" issue create --title <title>` three times.

- **Projects.** `$C pl "$RUN" project create Punchlist`, `$C pl "$RUN" project create Groundwork`, `$C pl "$RUN" milestone create "Gate 0" --project Punchlist --pass-when "pl creates and moves an issue"`, then `$C pl "$RUN" project list`. Rows: `$C sql "$RUN" "SELECT p.name, m.name, m.pass_when FROM project p LEFT JOIN milestone m ON m.project_id = p.id ORDER BY p.created_at, m.created_at"`.
- **Set and clear.** `$C pl "$RUN" issue edit PL-2 --parent PL-1 --project Punchlist --milestone "Gate 0"`, then `$C pl "$RUN" issue edit PL-2 --no-parent --no-project`. Rows: `$C sql "$RUN" "SELECT id, parent_id, project_id, milestone_id FROM issue ORDER BY number"` and `$C sql "$RUN" "SELECT seq, issue_id, kind, edited_fields FROM event WHERE kind = 'edited' ORDER BY seq"`.
- **Cycle.** With PL-2 under PL-1 and PL-3 under PL-2, `$C pl "$RUN" issue edit PL-1 --parent PL-3` exits 1 with the chain message, and `SELECT max(seq) FROM event` does not change.
- **Another workspace.** A second workspace needs `punchlist-server bootstrap` with another prefix against the run's database (`DATABASE_URL` from `run.env`); its token stays out of the transcript: `target/debug/punchlist-server --database-url "$DATABASE_URL" bootstrap --person Other --repository gannonh/other --prefix OT --server-url "$SERVER_URL" > /tmp/punchlist-verify/$RUN/other.toml`, then `target/debug/pl --config /tmp/punchlist-verify/$RUN/other.toml issue create --title other` prints `OT-1`. Then `$C curl "$RUN" -w ' %{http_code}' -X PATCH -H 'content-type: application/json' -d '{"parent":"OT-1"}' "$SERVER_URL/api/issues/PL-1"` answers `{"code":"invalid","message":"no issue OT-1 in this workspace"} 422`, and `pl --config …/other.toml issue show PL-1` exits 1 with `error: issue PL-1 is in another workspace (403 Forbidden)`.

## Gotchas

- `pl` looks project and milestone names up with `GET /api/projects` and sends their ids; the API takes ids, not names.
- `--milestone` without `--project` on an issue with no project fails in `pl` itself: `error: PL-1 has no project; give --project with --milestone`. So do a project the workspace lacks (`error: no project named Nope in this workspace`) and a milestone the project lacks (`error: project <project> has no milestone named <name>`); none sends a request, and each exits 1.
- clap refuses (exit 2) `--parent` with `--no-parent`, `--project` with `--no-project`, and `--milestone` with `--no-milestone` or `--no-project`: `error: the argument '--parent <PARENT>' cannot be used with '--no-parent'`.
- An older server ignores `parent`, `project` and `milestone` in a `PATCH` and answers 200, so check the answer's row, not only the exit code.
- The app picks existing projects and milestones; it cannot create them.
