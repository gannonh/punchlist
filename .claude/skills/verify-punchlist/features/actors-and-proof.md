# Agent actors and proof

A person mints a standalone agent actor with `pl actor create` and sees its token once; that token reaches `/api/issues/<id>` and below for the workspace's issues, `GET`/`POST /api/issues`, the event stream, `GET /api/projects`, `/api/workflow`, `/api/actors`, `/api/proof/{id}` and `GET /api/session` (who it is, so the web app can show an issue as that agent), and nothing else. `--name` names it (the orchestrator's is `Orchestrator`). It cannot sign a browser in. A person or an agent attaches a file to an issue as proof with `pl proof attach` (or the MCP tool `attach_proof`), and the server stores it with the issue's attempt at upload. `pl proof list` and `pl issue show` list it, and `GET /api/proof/{id}` serves the file to people, runners and standalone agents.

## Sub-features

- `actor-create` prints `Created agent <name>`, then `Id`, `Kind`, `Model` (when set), `Owner`, `Token` and `The token is shown once; only its hash is stored.` An unknown kind or owner answers 422; a token that is not a person's answers 403 `this needs a person's token`.
- `actor-list` prints `NAME  ROLE  KIND  MODEL  OWNER  ID` for the workspace's people, agents and runners, oldest first, `-` for an empty cell. Each claim's agent is listed too.
- `actor-scope`: a standalone agent's token on any route outside `/api/issues/<id>` other than `GET`/`POST /api/issues`, `GET /api/workspaces/<its id>/events`, `GET /api/projects`, `/api/workflow`, `/api/actors`, `/api/proof/<id>` and `/api/session` answers 403 `an agent acts only on the workspace's issues`; another workspace's stream or proof answers 404; `GET /api/session` answers its actor and workspace; `POST /api/session` with it answers 401. On an issue it answers as for a person, under the agent's locks and roles. A run's agent token answers 403 `an agent acts only on its run's issue, PL-<n>` everywhere else, `GET /api/session` included.
- `proof-attach` prints `Attached <uuid>: <kind> "<title>" (<size>) to <id> for attempt <n>`. Sizes are in powers of 1000 (`3 B`, `12.3 kB`).
- `proof-list` prints `No proof attached to <id>.` or one line per file, oldest first: `<time>  <kind>  "<title>"  <size>  <actor>  <uuid>`, the actor marked `Name (person)` or `Name (agent, <kind>)`. `pl issue show` prints the same lines under `Proof`, or `  None attached.`
- `proof-refusals`: an extension outside `PROOF_TYPES` fails in `pl` before any request; a `--kind` unlike the file's type answers 422; over `--proof-max-bytes` 413; an upload across a move to a new attempt 409 `stale_attempt`; a status locked to the uploader's role 409 `locked`.
- `proof-serve`: `GET /api/proof/{id}` answers the bytes with their content type, `x-content-type-options: nosniff` and `content-security-policy: sandbox`; an agent token gets 403, another workspace's file 404.

## How to get to it (user POV)

- `pl actor create --agent --kind <claude-code|codex|opencode|pi|cursor> [--model <m>] [--owner <person>]`, `pl actor list`; `POST /api/actors`, `GET /api/actors`.
- `pl proof attach <id> <path> --title <t> [--kind image|video|report]`, `pl proof list <id>`; `POST /api/issues/{id}/proof` (multipart), `GET /api/issues/{id}/proof`, `GET /api/proof/{id}`.

## Driving it with control-punchlist

Preconditions:

- Baseline run `$RUN`, `source /tmp/punchlist-verify/$RUN/run.env`, and `PL-1` created. No model calls and no GitHub writes.

- **Mint.** `$C pl "$RUN" actor create --agent --kind claude-code --model haiku > /tmp/punchlist-verify/$RUN/agent.out`. Exit `0`; the file holds `Created agent Claude Code`, `Kind   claude-code`, `Model  haiku`, `Owner  Verify Person`, a `Token  plt_…` line and `The token is shown once; only its hash is stored.` Read the token with `AG=$(sed -n 's/^Token *//p' /tmp/punchlist-verify/$RUN/agent.out)`.
- **Bad kind and owner.** `$C pl "$RUN" actor create --agent --kind gpt` exits `1` with ``error: unknown agent kind `gpt`; the kinds are claude-code, codex, opencode, pi, cursor (422 Unprocessable Entity)``. `--kind codex --owner Nobody` exits `1` with ``error: no person named `Nobody` in this workspace (422 Unprocessable Entity)``.
- **List.** `$C pl "$RUN" actor list` has a `Verify Person  person  -  -  -` row and a `Claude Code  agent  claude-code  haiku  Verify Person` row. Rows: `$C sql "$RUN" "SELECT name, role, kind, model FROM actor ORDER BY created_at"`.
- **Scope.** `curl -s -w ' %{http_code}' -H "Authorization: Bearer $AG" -X POST "$SERVER_URL/api/projects" -H 'content-type: application/json' -d '{"name":"p"}'` answers `{"code":"forbidden","message":"an agent acts only on the workspace's issues"} 403`; `GET` on `$SERVER_URL/api/actors` and `$SERVER_URL/api/issues/PL-1` answers 200, and on `$SERVER_URL/api/session` 200 with `{"actor":{…,"name":"Claude Code","role":"agent","kind":"claude-code"},"workspace_id":…}`. `PUNCHLIST_AGENT_TOKEN=$AG $C request-transition "$RUN" <an In Progress issue> agent_review` is refused at a gate, not for the agent ([issue-refusals.md](issue-refusals.md)).
- **Attach.** `printf 'ok\n' > $EVIDENCE/p.txt; $C pl "$RUN" proof attach PL-1 $EVIDENCE/p.txt --title Log`. Exit `0`, stdout `Attached <uuid>: report "Log" (3 B) to PL-1 for attempt 0`.
- **List and rows.** `$C pl "$RUN" proof list PL-1` prints one line ending `report  "Log"  3 B  Verify Person (person)  <uuid>`, and `pl issue show PL-1` has it under `Proof`. `$C sql "$RUN" "SELECT kind, title, content_type, size, attempt FROM proof"` is `report | Log | text/plain | 3 | 0`. The file is under `/tmp/punchlist-verify/$RUN/proof/`.
- **Serve.** `$C curl "$RUN" -i "$SERVER_URL/api/proof/<uuid>"` answers `HTTP/1.1 200 OK`, `content-type: text/plain`, `x-content-type-options: nosniff`, `content-security-policy: sandbox` and body `ok`. With `-H "Authorization: Bearer $AG"` through plain `curl` it answers 403.
- **Refusals.** `cp $EVIDENCE/p.txt $EVIDENCE/x.exe; $C pl "$RUN" proof attach PL-1 $EVIDENCE/x.exe --title t` exits `1` with `error: <path> is not a file proof can be; use one of .png, .jpg, .jpeg, .gif, .webp, .mp4, .webm, .mov, .txt, .log, .md, .xml, .pdf`. `proof attach PL-1 $EVIDENCE/p.txt --title t --kind image` exits `1` with ``error: `text/plain` is report proof, not `image` (422 Unprocessable Entity)``. `SELECT count(*) FROM proof` is still `1`.

## Gotchas

- `$C pl` records stdout, so `actor create`'s token is in `transcript.txt` even when stdout is redirected. The token is only good against the run's database, which `down` drops; still, never quote that transcript entry.
- Proof's `attempt` counts the issue's moves from Start to In Progress (0 before the first), not the `attempt` table's per-run number.
- `up` stores each run's uploads under `/tmp/punchlist-verify/<RUN_ID>/proof`, which `down` deletes. A server started another way writes to `~/.local/share/punchlist/proof`.
- A run's own agent token works only on its run's issue while the run runs; a standalone agent's works on any issue in the workspace and does not lapse.
