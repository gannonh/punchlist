# Punchlist verification map

This directory is the maintained source for verifying Punchlist's user-facing behavior. Read this index, then use the matching feature file as the recipe. `C` below is `.claude/skills/verify-punchlist/bin/control-punchlist`, run from the repository root.

| Feature | File | Entry points |
| --- | --- | --- |
| Create and show an issue | [issue-create-show.md](issue-create-show.md) | `pl issue create`, `pl issue show`, `POST /api/issues`, `GET /api/issues/{id}` |
| Move an issue and read its timeline | [issue-move.md](issue-move.md) | `pl issue move`, `pl issue show`, `POST /api/issues/{id}/transitions`, `GET /api/issues/{id}/events` |
| Refused moves | [issue-refusals.md](issue-refusals.md) | `pl issue move`, `POST /api/issues/{id}/transitions` |
| List issues | [issue-list.md](issue-list.md) | `pl issue list`, `GET /api/issues` |
| Runners, claims and runs | [runner.md](runner.md) | `pl runner start`, `pl runner list`, `pl issue show` (runs), `pl run log`, `/api/runners`, `/api/runs/*` |
| GitHub webhook, pull requests and checks | [github.md](github.md) | `$C webhook`, `POST /api/github/webhook`, `pl issue show` (pull requests, comments), `pl pr unlinked`, `/api/issues/{id}/pull-requests`, `/api/pull-requests/unlinked` |
| Workflow file, gated transitions and the agent's MCP tools | [workflow-and-gates.md](workflow-and-gates.md) | `pl workflow show`, `pl issue transition`, `pl issue comment`, `pl mcp`, `/api/workflow`, `/api/workflow/schema.json`, `POST /api/issues/{id}/comments` |
| API document and auth | [api-and-auth.md](api-and-auth.md) | `GET /api/openapi.json`, any `/api/issues` route without a valid token, `pl` without a config |

## Baseline preconditions

- A run started with `RUN=$($C up)` and `$C doctor "$RUN"` printing only `ok` lines.
- The run's database is fresh: the next issue is `PL-1`, and one actor exists, `Verify Person` with role `person`.
- The `job` table is not empty: `bootstrap` queues one `workflow_load` job. Count webhook jobs with `WHERE kind = 'github_event'`.
- Never drive an instance this run did not start.

## Driving conventions

- Run `pl` only through `$C pl "$RUN" ...` so the transcript records it.
- Run SQL only through `$C sql "$RUN" "..."`. Reads only; never write rows to set up a state a user could reach with `pl`. It prints psql's aligned table (a header, a rule, the rows, then `(n rows)`); the recipes quote only the rows, as `a | b`.
- Treat every command as literal. Status arguments are workflow keys such as `in_progress`, not display names.
- The person actor can make only `backlog → todo`, `todo → start` and `human_review → merging`. `start → in_progress` is made by a runner started with `$C runner` (see [runner.md](runner.md)). Later statuses need an agent actor: each claim creates one for its run, and only that run's `pl mcp` holds its token (see [workflow-and-gates.md](workflow-and-gates.md)). Do not insert actors by hand to get there.

## Proof and skip reporting

- CLI proof is the transcript entry: command, stdout, stderr and exit code.
- Every write is followed by a second read: `pl issue show` and the SQL rows.
- Record the feature file and entry point used with each quoted artifact.
- Report an unreachable entry point as skipped, with the command tried and the unmet precondition. Do not count it as verified through another entry point.

## Feature entry contract

Each feature file starts with an H1 and one paragraph describing the user-visible behavior, then exactly four H2s in this order: `Sub-features`, `How to get to it (user POV)`, `Driving it with control-punchlist` (starting with `Preconditions:`), and `Gotchas`.
