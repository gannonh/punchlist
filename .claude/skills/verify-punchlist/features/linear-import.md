# Linear import

A person imports one Linear project into their workspace with `pl import linear`. `pl` reads Linear with the personal key in `LINEAR_API_KEY` and posts what it read; the server's `linear_import` job writes the issues (Linear's state mapped to a status), their labels, project, milestone, parent and comments, plus the people who wrote them. A rerun writes nothing twice: it brings in new issues and comments, updates issues Linear changed since, and reports ones gone from the project. `pl import status` prints what the workspace has from Linear and how the last import went.

## Sub-features

- `import`: `pl import linear --team <key> --project <name> --workspace <id>` prints `Read KAT/Punchlist from Linear: 3 issues, 2 comments, 2 milestones in 0.0s.`, `Import <job id> done.`, a Created/Updated table for Issues, Comments, Projects, Milestones and People, any `Notes`, and `Elapsed <time>.`; exit 0.
- `rerun`: the same command again prints `0` in every Created and Updated cell, and `SELECT count(*) FROM issue WHERE linear_id IS NOT NULL` does not change.
- `mapping`: a Linear state goes to the `[import.linear.states]` entry, else the status of the same name (any case), else by type (backlog, unstarted → todo, started → in_progress, completed → done, canceled and duplicate → canceled). An unmapped state (Triage) answers 422: `error: Linear state "Triage" maps to no status; map it under [import.linear.states] in .punchlist/workflow.toml (422 Unprocessable Entity)`, and no `job` row is written.
- `actors`: each Linear user becomes a person with no token (`token_hash` NULL, `linear_id` set); comments whose last line is `(agent)`, or with no user, belong to `Imported agent` (role `agent`, kind `imported`). `pl issue show` marks them `Imported agent (agent, imported)` and prints `Linear: KAT-3743  <url>`.
- `update`: an issue whose Linear `updatedAt` is newer than its `linear_updated_at` gets one `edited` event naming the changed fields and, for a new state, a `transition` row by the importer; no `dispatch` row.
- `notes`: `KAT-12 (PL-7) is no longer in Linear's project`, `KAT-3 (PL-1) has a parent outside Linear's project, so it has none here`, `KAT-2 (PL-3): skipped an empty comment from <time>`.
- `status`: `pl import status` prints `Imported from Linear`, the totals of rows with a Linear id, then `Last import <id>  <state>  <time>` with its table, notes or `Error:`; `No imports yet.` before the first.
- `errors`: without `LINEAR_API_KEY`: `error: LINEAR_API_KEY is not set; …`; a key Linear refuses: `error: Linear refused the query: <Linear's message>`; both exit 1 and send nothing to the server.

## How to get to it (user POV)

- `pl import linear`, `pl import status`, `pl issue show`, `pl issue list`.
- `POST /api/workspaces/{id}/imports/linear` (a `LinearImport` body), `GET /api/workspaces/{id}/imports/{run}`, `GET /api/workspaces/{id}/imports`: the token's workspace (else 404), people only (else 403).

## Driving it with control-punchlist

Preconditions:

- Baseline run `$RUN`; the workspace id from `$C sql "$RUN" "SELECT id FROM workspace"`.
- Real Linear needs `LINEAR_API_KEY` in the Punchlist 1Password Environment for an enabled Linear account: `with-env $C pl "$RUN" import linear --team KAT --project Punchlist --workspace <id>`. Without one, `bin/fake-linear` stands in for Linear.

- **Real import, twice.** `with-env $C pl "$RUN" import linear --team KAT --project Punchlist --workspace <id>` twice, `$C pl "$RUN" import status` after each, and `$C sql "$RUN" "SELECT count(*) FROM issue WHERE linear_id IS NOT NULL"`, which must equal Linear's issue count for the project.
- **Without Linear.** `.claude/skills/verify-punchlist/bin/fake-linear 47611 &` serves `crates/punchlist-cli/tests/fixtures/linear/` (KAT-3743, KAT-3744 and KAT-3782, two milestones, two `(agent)` comments); then `LINEAR_API_KEY=lin_api_test LINEAR_API_URL=http://127.0.0.1:47611/graphql $C pl "$RUN" import linear --team KAT --project Punchlist --workspace <id>`. Any other key gets Linear's real `Account disabled` answer.
- **300 issues in under 10 minutes.** `bin/fake-linear 47612 --issues 300 &` serves 300 generated issues with 3 comments each in pages of 50; import with `--team GEN --project Generated` against port 47612 and read `Elapsed`. Rows: `$C sql "$RUN" "SELECT p.name, count(i.*) FROM project p LEFT JOIN issue i ON i.project_id = p.id GROUP BY p.name"`.
- **Rows.** `$C sql "$RUN" "SELECT id, linear_identifier, status, parent_id, milestone_id FROM issue ORDER BY number"`, `$C sql "$RUN" "SELECT name, role, kind, token_hash IS NULL FROM actor"`, `$C sql "$RUN" "SELECT id, state, attempts, last_error, result FROM job WHERE kind = 'linear_import' ORDER BY id"`.

## Gotchas

- Stop `fake-linear` by its PID (`kill %1`, or `pgrep -f 'fake-linear 4761'`), not `pkill -f fake-linear`: that pattern also matches the shell running the command.
- The import's id is the `job` id, so the first is 2: bootstrap's `workflow_load` is job 1.
- `Elapsed` counts in `pl`'s 0.5 s polls; the job itself is usually done before the first.
- A rerun ignores an issue whose `updatedAt` did not move, even if someone edited it in Punchlist; edited Linear comments are not updated.
- An issue in a status another workflow load drops would block that load ("status in use is not in the workflow"), as for any issue.
