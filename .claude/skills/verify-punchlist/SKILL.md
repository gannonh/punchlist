---
name: verify-punchlist
description: Launch and drive Punchlist (the punchlist-server API on Postgres and the `pl` CLI) to prove a change works for live checks. Use before a Punchlist PR leaves draft, when rerunning another agent's live checks, or whenever you need evidence that `pl issue create/show/list/move` or the server's transition rules behave as the Linear issue says. Names the sandbox repository and the model for live checks.
---

# Verify Punchlist

Punchlist today is a server (`punchlist-server`, axum on Postgres) and a terminal client (`pl`). A user drives it by running `pl` commands against a running server. There is no web app, runner or GitHub integration yet; extend this skill when a slice adds one.

Everything here runs from the repository root on sartre.

## Fixed facts for live checks

- **Sandbox repository:** `gannonh/punchlist-sandbox` (private, default branch `main`). Live checks that open branches or pull requests use this repository, never `gannonh/punchlist` or any other real repository.
- **Live-check model:** agent runs in live checks use Claude Code with `--model haiku`, the cheapest Claude model. Make a live model call only inside a named live check.
- **Secrets:** nothing goes in a `.env` file. A command that needs a secret (GitHub App credentials, `LINEAR_API_KEY`) runs under `with-env`, which loads the Punchlist 1Password Environment for that one process: `with-env <command>`. `with-env --names` lists the variable names. Never print secret values. Slice 1 needs no secrets.

## Launch

`bin/control-punchlist up [RUN_ID]` starts an isolated instance and prints its run id on stdout:

```sh
C=.claude/skills/verify-punchlist/bin/control-punchlist
RUN=$($C up)            # or: RUN=$($C up my-check)
```

It:

1. Starts the dev Postgres container with `docker compose up -d --wait db` (`127.0.0.1:5433`, shared by every run).
2. Creates a fresh database `punchlist_verify_<RUN_ID>` in it, so runs never share data.
3. Builds `punchlist-server` and `pl` with `SQLX_OFFLINE=true`.
4. Runs `pnpm dev` with `DATABASE_URL` pointing at the fresh database and `PUNCHLIST_BIND` on a free port, in its own process group. The server runs migrations at start.
5. Waits for `listening on http://127.0.0.1:<port>` in the server log (up to 60 s).
6. Runs `punchlist-server bootstrap --person "Verify Person"`, which creates workspace `Punchlist` (prefix `PL`), repository `gannonh/punchlist` and one person actor, and writes the `pl` config with that person's token.

Instance state lives in `/tmp/punchlist-verify/<RUN_ID>/` (`run.env`, `config.toml`, `server.log`, `server.pgid`). `source /tmp/punchlist-verify/$RUN/run.env` gives `SERVER_URL`, `DATABASE_URL`, `CONFIG` and `EVIDENCE`.

A live check that says "start with `pnpm dev`" is satisfied by `up`: it launches through `pnpm dev`, with only the database and port overridden for isolation. If a check needs the default dev instance itself (`127.0.0.1:7878`, database `punchlist`), first make sure nobody else is using it: `ss -ltnp | grep 7878` must show nothing.

## Doctor

Run this first, and again whenever anything looks off:

```sh
$C doctor "$RUN"
```

It is read-only and checks that the run's process group is alive, the server log shows the run's own URL, `/api/openapi.json` answers 200, the run's token is accepted by `GET /api/issues`, and the run's database answers. It prints the `pl` version and the git commit. Any `FAIL` line means do not drive this instance: run `down` and `up` again.

## Drive

Run `pl` as the bootstrapped person, with the transcript recorded:

```sh
$C pl "$RUN" issue create --title "First" --body "Body"     # prints PL-1
$C pl "$RUN" issue show PL-1
$C pl "$RUN" issue list
$C pl "$RUN" issue move PL-1 todo
```

`pl` arguments pass through unchanged, and the command's stdout, stderr and exit code pass through too, so `$?` is the real exit code. Status arguments are workflow keys: `backlog`, `todo`, `start`, `in_progress`, `agent_review`, `human_review`, `merging`, `done`, `canceled`. Issue ids are `PL-<n>`, numbered from 1 in each run.

Read the stored rows behind any claim:

```sh
$C sql "$RUN" "SELECT id, status FROM issue"
$C sql "$RUN" "SELECT from_status, to_status, actor_id FROM transition ORDER BY created_at"
$C sql "$RUN" "SELECT seq, issue_id, kind, actor_id FROM event ORDER BY seq"
$C sql "$RUN" "SELECT id, name, role FROM actor"
```

Raw HTTP, for API checks: `source /tmp/punchlist-verify/$RUN/run.env`, then `$C curl "$RUN" "$SERVER_URL/api/issues"`. It runs `curl -s` with the run's token passed on stdin, so the token stays out of process listings; any other curl arguments pass through. Do not paste the token into evidence.

The feature map in `features/` has a recipe per feature. Read `features/README.md` first.

## Evidence

Every `$C pl` and `$C sql` call appends the command, stdout, stderr and exit code to `verify-artifacts/<RUN_ID>/transcript.txt` in the repository root. `down` copies the server log there as `server.log`. `verify-artifacts/` is gitignored; quote from the transcript in the PR body, or commit chosen files to the `assets/<issue-id>` branch.

Proof standards:

- Drive the real path: `pl` against a running server. Do not call handlers, insert rows or use test helpers to stand in for a user action.
- Capture the action and the resulting state: the `pl` command and its output, then a second read (`pl issue show`, or the SQL rows) that shows what changed.
- A write needs its rows: include the query and the rows behind every number or status you claim.
- A refusal needs three things: a non-zero exit code, the message naming the rule, and a SQL read showing no new `transition` row.
- Pass or fail comes from comparing output to the check's literal "Pass when", not from reading it loosely.
- A check you could not run is reported as skipped with the command you tried. It never passes through another path.

## Cleanup

```sh
$C down "$RUN"
```

It stops the run's own process group (`pnpm dev`, cargo and the server; never anything by name), drops the run's database, deletes `/tmp/punchlist-verify/<RUN_ID>/`, and leaves `verify-artifacts/<RUN_ID>/` in place. The shared Postgres container keeps running; stop it with `docker compose down` only if you started it and nothing else uses it. Run `down` after a failed run too, so no server or database is left behind.

## Main versus branch

Check 1 of every PR runs the same flow on `main` and on the branch. Use a separate worktree for `main` so the branch checkout is untouched:

```sh
git worktree add /tmp/punchlist-main origin/main
(cd /tmp/punchlist-main && <the same command>)
git worktree remove /tmp/punchlist-main
```

On `main` before Slice 1 merges there are no crates, so cargo commands there fail; quote that failure as the `main` half.
