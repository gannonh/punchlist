# Workflow file, gated transitions and the agent's tools

The server loads `.punchlist/workflow.toml` and the prompts it names from the repository's default branch, as the GitHub App: once when `punchlist-server bootstrap` queues a load, and again on every push to the default branch that changes `.punchlist/`. Each valid version is stored by content hash in `workflow_version` and becomes active; an invalid one is refused with its line and key in the server log, and the previous version stays active. Until one loads, the workspace uses the built-in workflow. Every transition checks the lock, the role and the gates in one transaction, and records each gate's result in `transition_gate`. A failed gate answers 409 with `gate`, `reason`, `evidence` and every gate's result. Each claim creates an agent actor for its run; the runner hands its token to `pl mcp`, which Claude Code gets as the `punchlist` MCP server with the tools `get_issue`, `comment` and `request_transition`.

## Sub-features

- `workflow-show`: `pl workflow show` prints `Version   sha256:<hex>`, `Commit    <sha>` (or `built-in`) and `Statuses  backlog, todo, start, in_progress, agent_review, human_review, merging, done, canceled`.
- `workflow-load`: a push to `main` that changes `.punchlist/` adds one `workflow_version` row and makes it active. The server log says `loaded workflow sha256:… from gannonh/punchlist-sandbox main at <sha>`.
- `workflow-refused`: a file that does not load leaves `workflow_version` and `pl workflow show` unchanged. The server log says ``refused .punchlist/workflow.toml on <repository> <branch> at <sha>: line <n>, `<key>`: <problem>``, such as ``line 26, `transition[3].gates[1]`: unknown gate `pr_redy`; gates are …``.
- `schema`: `GET /api/workflow/schema.json` answers without a token with the JSON Schema; its `title` is `Punchlist workflow`.
- `gate-refused`: `request_transition` (or `pl issue transition`) to `agent_review` while the pull request is a draft answers `Refused (gate_failed): in_progress → agent_review needs gate pr_ready: pull request #<n> is a draft`, then each gate's result. No `transition` row is written.
- `gate-passed`: after `gh pr ready`, the same request moves the issue to Agent Review as `Claude Code (agent)`, and `transition_gate` holds `pr_open`, `pr_ready`, `pr_names_issue`, all `pass`.
- `agent-comment`: a comment through MCP ends with `(agent)` on its own line in `pl issue show`.
- `agent-scope`: an agent token works only while its run is running, only on `/api/issues/<its run's issue>` and below (any other endpoint answers 403 `an agent acts only on its run's issue, PL-<n>`), and not on an issue in a status the workflow locks to agents (`human_review`).

## How to get to it (user POV)

- `pl workflow show`; `GET /api/workflow`; `GET /api/workflow/schema.json`.
- `pl issue transition <id> <status>` (alias of `pl issue move`); `pl issue comment <id> --body <text>`; `POST /api/issues/{id}/transitions`; `POST /api/issues/{id}/comments`.
- In a Claude Code run started by `pl runner start`: the MCP tools `mcp__punchlist__get_issue`, `mcp__punchlist__comment` and `mcp__punchlist__request_transition`.

## Driving it with control-punchlist

Preconditions:

- The GitHub App has **Contents: Read-only** and subscribes to **Push**, on top of what [github.md](github.md) needs.
- The run reads the sandbox as the App: `PUNCHLIST_VERIFY_PORT=7879 with-env $C up`. `with-env` supplies `GITHUB_APP_PRIVATE_KEY` and the webhook secret; `up` sets `GITHUB_APP_ID=5254938` when the key is present. Without the key the server log says `no GitHub App credentials`, the load job fails with `no GitHub App is configured`, and every check that needs a load is skipped.
- The sandbox's `main` carries the PRD's `workflow.toml` and the prompts in `.punchlist/prompts/` (copy them from this repository's `.punchlist/`).

- **Loaded at bootstrap.** After `up`, poll `$C sql "$RUN" "SELECT state FROM job WHERE kind = 'workflow_load'"` until `done`. `$C pl "$RUN" workflow show` prints the hash and nine statuses, and `$C sql "$RUN" "SELECT hash, commit_sha FROM workflow_version"` has one row whose hash matches. On `main` before this slice, `pl workflow show` fails with clap's `unrecognized subcommand 'workflow'`.
- **Refused.** Push a commit to the sandbox's `main` that renames a gate in `.punchlist/workflow.toml` (such as `pr_ready` to `pr_redy`). Wait for the push delivery's job (`SELECT kind, state FROM job ORDER BY id`). `pl workflow show` prints the same hash, `SELECT count(*) FROM workflow_version` is unchanged, and `grep refused /tmp/punchlist-verify/$RUN/server.log` names the line and key. Revert the commit afterwards; the revert loads back the earlier hash without a new row.
- **Gate refused, then passed.** Start a runner (`$C runner "$RUN" r1`), create an issue whose body tells the agent to open a draft pull request, request Agent Review through `request_transition` while it is still a draft, then run `gh pr ready` and request again. In `$C pl "$RUN" run log <run-id>`, find the `tool_result` for the first `mcp__punchlist__request_transition`: it names `pr_ready` and `pull request #<n> is a draft`. Then `$C pl "$RUN" issue show PL-1` shows `Claude Code (agent)  In Progress → Agent Review`, and `$C sql "$RUN" "SELECT g.gate, g.result FROM transition_gate g JOIN transition t ON t.id = g.transition_id WHERE t.to_status = 'agent_review' ORDER BY g.position"` lists the three gates as `pass`.
- **Agent comment.** Have the run call `comment`; `pl issue show PL-1` shows `Claude Code (agent)  commented:` with `(agent)` as the last line.

## Gotchas

- The load reads the default branch's head when the job runs, not the commit the push named, so two quick pushes both load the newest file.
- `deny_unknown_fields` is on: a misspelt key, such as `gate` for `gates`, is refused rather than loaded without its gates.
- A refused file leaves no row anywhere; the server log is its only record.
- A gated request reads the issue's pull requests from GitHub first, as the App, because webhooks reach the server seconds after GitHub. A request made right after `gh pr create` or `gh pr ready` therefore sees the new state. If GitHub cannot be read, the server log says `cannot read pull requests from GitHub, the gates read the recorded ones` and the recorded pull requests decide. A pull request GitHub reports closed or merged is left to its webhook.
- The agent token ends with its run. To re-request after the run finished, start a new run; a person's token cannot make `in_progress → agent_review`.
