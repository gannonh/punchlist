---
name: verify-punchlist
description: Launch and drive Punchlist (the punchlist-server API on Postgres, the `pl` CLI and the runner) to prove a change works for live checks. Use before a Punchlist PR leaves draft, when rerunning another agent's live checks, or whenever you need evidence that `pl issue create/show/list/move`, `pl runner start/list`, `pl run log` or the server's transition and claim rules behave as the Linear issue says. Names the sandbox repository and the model for live checks.
---

# Verify Punchlist

Punchlist today is a server (`punchlist-server`, axum on Postgres) that also serves the web app, a terminal client (`pl`) and a runner (`pl runner start`) that claims issues in Start and runs Claude Code on them, then claims them again in Agent Review and runs Codex. A user drives it by running `pl` commands against a running server. The server also takes GitHub App webhook deliveries at `POST /api/github/webhook`; [features/github.md](features/github.md) shows how to send signed fixture payloads and what to read afterwards. The server also loads `.punchlist/workflow.toml` from the sandbox's default branch, and agents request transitions through `pl mcp`; [features/workflow-and-gates.md](features/workflow-and-gates.md) covers both. A person drives the web app in a browser; [Web app](#web-app) says how.

Everything here runs from the repository root on sartre.

## Fixed facts for live checks

- **Sandbox repository:** `gannonh/punchlist-sandbox` (private, default branch `main`). Live checks that open branches or pull requests use this repository, never `gannonh/punchlist` or any other real repository.
- **Live-check models:** Claude Code runs in live checks use the `haiku` alias (`--model haiku`), which tracks the newest Haiku and is Haiku 5.5 (`claude-haiku-5-5`) on the Anthropic API; on Bedrock, Vertex, Foundry and Claude Platform on AWS it is still Haiku 4.5, and it needs Claude Code 2.1.293 or later. A run's log shows the model it resolved to (`"model":"claude-haiku-5-5"`); a live check on a Haiku older than 5.5 does not count. Codex runs use GPT-6 Luna, `gpt-6-luna` (`-m gpt-6-luna`), which OpenAI documents without an alias, so it is the model's own slug; not the older `gpt-5.6-luna`. Use no other model for a live check. Make a live model call only inside a named live check. `$C runner` passes both: `--model haiku --codex-model gpt-6-luna`. A Codex run's log shows the model its thread started with (`"model":"gpt-6-luna"` in the `thread/started` line).
- **Scratch files:** every file a check writes outside `verify-artifacts/` goes under `$TMPDIR`, never a fixed `/tmp/...` path: the instance, sandbox clones, the `main` worktree and harness output. In a runner's attempt `$TMPDIR` is the attempt's own directory on disk, `<state dir>/tmp/<runner id>/<run id>-<attempt>`, and the runner removes it when the attempt ends; `/tmp` on sartre is a tmpfs held in memory. Outside a run, set it first: `export TMPDIR=${TMPDIR:-/tmp}`.
- **Secrets:** nothing goes in a `.env` file. A command that needs a secret (GitHub App credentials, `LINEAR_API_KEY`) runs under `with-env`, which loads the Punchlist 1Password Environment for that one process: `with-env <command>`. `with-env --names` lists the variable names. Never print secret values. Fixture webhook deliveries need no secret from 1Password: `up` generates a random webhook secret per run. Real GitHub deliveries need the GitHub App's secret, and loading `.punchlist/` from the sandbox needs its private key, so start those runs under `with-env $C up`.

## Sandbox GitHub

A run's own `gh` and git act as the workspace's GitHub App with a token for `gannonh/punchlist` alone, so they cannot see the sandbox: `gh pr list --repo gannonh/punchlist-sandbox` answers `Could not resolve to a Repository`. Every command on the sandbox's GitHub goes through `control-punchlist` under `with-env`, which mints an installation token of the sandbox App (`punchlist-gannonh`, id 5254938) for `gannonh/punchlist-sandbox` alone:

```sh
with-env $C gh pr list --repo gannonh/punchlist-sandbox        # gh with GH_TOKEN set to the sandbox token
with-env $C gh api graphql -f query='mutation { resolveReviewThread(input: {threadId: "<id>"}) { thread { isResolved } } }'
with-env $C git clone https://github.com/gannonh/punchlist-sandbox.git $TMPDIR/sandbox-$RUN
with-env $C git -C $TMPDIR/sandbox-$RUN push -u origin <branch>   # the token is git's credential for the sandbox's URL only
```

`with-env` picks the 1Password Environment by the current repository, so run these from this checkout's root and point git at a sandbox clone with `-C`; from inside the clone `with-env` answers `no Environment for 'punchlist-sandbox'`. The token lasts an hour and is cached in `$TMPDIR/punchlist-verify/sandbox-token.json` (0600); a call mints a new one when it has under 10 minutes left. Nothing prints it except `with-env $C sandbox-token`, so never run that into evidence. The token cannot write to `gannonh/punchlist` (a write answers 403; the repository is public, so reads answer as they would for anyone). `with-env bin/check-sandbox-token` checks all of this.

## Launch

`bin/control-punchlist up [RUN_ID]` starts an isolated instance and prints its run id on stdout:

```sh
C=.claude/skills/verify-punchlist/bin/control-punchlist
RUN=$($C up)            # or: RUN=$($C up my-check)
```

It:

1. Starts the dev Postgres container with `docker compose up -d --wait db` (`127.0.0.1:5433`, shared by every run). Inside a runner's attempt the container carries the label `punchlist.attempt=$PUNCHLIST_ATTEMPT`, and the runner removes it when the attempt ends, with every process that carries the attempt's `PUNCHLIST_ATTEMPT`, so a check need not stop it.
2. Creates a fresh database `punchlist_verify_<RUN_ID>` in it, so runs never share data.
3. Builds `punchlist-server` and `pl` with `SQLX_OFFLINE=true`.
4. Runs `pnpm dev` with `DATABASE_URL` pointing at the fresh database, `GITHUB_WEBHOOK_SECRET` (the environment's value if set, else a random one; kept in the 0600 file `webhook-secret` in the state directory, never printed) `PUNCHLIST_BIND` on a free port (on `PUNCHLIST_VERIFY_HOST` when set, else 127.0.0.1), `PUNCHLIST_WEB_DIR` on the run's own directory linking `web/dist`, `PUNCHLIST_PROOF_DIR` in the run's directory, and `PUNCHLIST_DEV_VITE=0`, in its own process group. The server runs migrations at start.
5. Waits for `listening on http://127.0.0.1:<port>` in the server log (up to 60 s).
6. Runs `punchlist-server bootstrap --person "Verify Person" --repository gannonh/punchlist-sandbox`, which creates workspace `Punchlist` (prefix `PL`), the sandbox repository and one person actor, and writes the `pl` config with that person's token.

Instance state lives in `$TMPDIR/punchlist-verify/<RUN_ID>/` (`run.env`, `config.toml`, `server.log`, `server.pgid`, `proof/`). `source $TMPDIR/punchlist-verify/$RUN/run.env` gives `SERVER_URL`, `DATABASE_URL`, `CONFIG` and `EVIDENCE`.

A live check that says "start with `pnpm dev`" is satisfied by `up`: it launches through `pnpm dev`, with only the database and port overridden for isolation. If a check needs the default dev instance itself (`127.0.0.1:7878`, database `punchlist`), first make sure nobody else is using it: `ss -ltnp | grep 7878` must show nothing.

Another worktree's Postgres container may already hold `127.0.0.1:5433`, and then `up` fails at `docker compose up` with a port-in-use error. Check with `docker ps --format '{{.Names}} {{.Ports}}' | grep 5433`. Don't stop another worktree's container; give this worktree its own port instead: `export PUNCHLIST_DB_PORT=5434` (any free port) before `up`, `down` and `pnpm check`.

`$C restart "$RUN"` stops the run's server and starts it again on the same database and port, as a crash or a deploy would; the old log moves to `server.log.<time>`. Run it under `with-env` to keep the GitHub App's credentials. A run off port 7879 gets no GitHub deliveries, so a push to the sandbox before `restart` is a push the server missed while down.

## Doctor

Run this first, and again whenever anything looks off:

```sh
$C doctor "$RUN"
```

It is read-only and checks that the run's process group is alive, the server log shows the run's own URL, `/api/openapi.json` answers 200, the run's token is accepted by `GET /api/session` (which needs no workspace, so it answers for an account in several), and the run's database answers. It prints the `pl` version and the git commit. Any `FAIL` line means do not drive this instance: run `down` and `up` again.

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

## Web app

The run's server serves the build in `web/dist` at `SERVER_URL`. Build it first; `up` links the build, so a rebuild shows on reload:

```sh
pnpm build
RUN=$(PUNCHLIST_VERIFY_HOST=$(tailscale ip -4) $C up)   # reachable from the Mac mini; plain `$C up` for headless checks on sartre
URL=$($C sign-in-url "$RUN")                           # a page that signs the browser in as the run's person
```

`sign-in-url` writes a page with a random name into the run's web directory and prints its URL. The page posts the person's token to `/api/session` and goes to `/`, which opens the person's last workspace, `/w/<prefix>`; the token never appears in the URL or the transcript. The page is removed after 2 minutes, so get one per browser.

Drive it one of two ways:

- **T3 Code integrated browser** (it runs on the Mac mini, so the run must be bound to the Tailscale address). Load the tools with ToolSearch `select:mcp__t3-code__preview_open,mcp__t3-code__preview_click,mcp__t3-code__preview_evaluate,mcp__t3-code__preview_snapshot,mcp__t3-code__preview_type,mcp__t3-code__preview_press,mcp__t3-code__preview_resize,mcp__t3-code__preview_recording_start,mcp__t3-code__preview_recording_stop`, then `preview_open` the URL. Assert with `preview_evaluate` on the DOM, not by looking at a screenshot.
- **Headless Chromium on sartre**, through Playwright from this repository (`node_modules/.pnpm/node_modules/playwright`), for scripted checks and timings. Launch it with `chromium.launch({ ignoreDefaultArgs: ['--disable-dev-shm-usage'] })`, as `web/playwright.config.ts` does: Playwright's default puts the renderer's shared memory in `/tmp`, a quota-limited tmpfs on sartre, and renderers crash when that quota fills.

[features/web-app.md](features/web-app.md) lists the selectors, the keys, the render measure and the recipes, including an In Progress issue without a model call. `pnpm e2e` runs `web/e2e/` against a run of its own.

`pnpm dev` also serves the app from Vite (`127.0.0.1:5173`, proxying `/api`); `PUNCHLIST_VITE_HOST=$(tailscale ip -4) pnpm dev` binds it for the integrated browser. `up` runs `pnpm dev` with `PUNCHLIST_DEV_VITE=0`, so a run starts no Vite.

## Runners

A runner makes live model calls once it claims an issue, so start one only inside a named live check:

```sh
$C runner "$RUN" r1              # pl runner start --name r1 --model haiku --codex-model gpt-6-luna, its own worktree root
$C runner "$RUN" r1 --agents claude-code,codex   # a runner that also claims Codex dispatches
$C pl "$RUN" runner list
$C pl "$RUN" run log <run-id>
$C runner-stop "$RUN" r1         # SIGINT; running attempts finish failed, "runner stopped"
```

The runner's own log is `verify-artifacts/<RUN_ID>/runner-<name>.log`. Its state directory, with its clones and worktrees, is `$TMPDIR/punchlist-verify/<RUN_ID>/runner/<name>/`. Recipes, including two runners racing for one issue, are in [features/runner.md](features/runner.md).

A GitHub delivery, signed with the run's webhook secret: `$C webhook "$RUN" pull_request payload.json [DELIVERY_ID]` prints the HTTP status and records it in the transcript. Recipes are in [features/github.md](features/github.md).

What an agent's `get_issue` prints, without a model: `$C get-issue "$RUN" PL-1` calls the MCP tool through `pl mcp` with the run's person token and records the text ([features/workflow-and-gates.md](features/workflow-and-gates.md)).

Raw HTTP, for API checks: `source $TMPDIR/punchlist-verify/$RUN/run.env`, then `$C curl "$RUN" "$SERVER_URL/api/issues"`. It runs `curl -s` with the run's token passed on stdin, so the token stays out of process listings; any other curl arguments pass through. Do not paste the token into evidence.

`$C fake-github "$RUN"` starts `bin/fake-github` and restarts the run's server with GitHub sign-in on, so the web app's `Sign in with GitHub` and `Link GitHub` can be driven ([features/sign-in.md](features/sign-in.md)).

`pl import linear` reads Linear with `LINEAR_API_KEY` (run it under `with-env`); `bin/fake-linear` stands in for Linear when the key cannot reach it ([features/linear-import.md](features/linear-import.md)).

The feature map in `features/` has a recipe per feature. Read `features/README.md` first.

## Evidence

Every `$C pl`, `$C sql` and `$C webhook` call appends the command, stdout, stderr and exit code to `verify-artifacts/<RUN_ID>/transcript.txt` in the repository root. `down` copies the server log there as `server.log`. `verify-artifacts/` is gitignored; quote from the transcript in the PR body, or commit chosen files to the `assets/<issue-id>` branch.

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

It stops the run's runners and its own process group (`pnpm dev`, cargo and the server; never anything by name), drops the run's database, deletes `$TMPDIR/punchlist-verify/<RUN_ID>/`, and leaves `verify-artifacts/<RUN_ID>/` in place. The shared Postgres container keeps running until the attempt ends (outside a run, stop it with `docker compose down` only if you started it and nothing else uses it). Run `down` after a failed run too, so no server or database is left behind.

## Main versus branch

Check 1 of every PR runs the same flow on `main` and on the branch. Use a separate worktree for `main` so the branch checkout is untouched:

```sh
git worktree add $TMPDIR/punchlist-main origin/main
(cd $TMPDIR/punchlist-main && <the same command>)
git worktree remove $TMPDIR/punchlist-main
```

On `main` before Slice 1 merges there are no crates, so cargo commands there fail; quote that failure as the `main` half.
