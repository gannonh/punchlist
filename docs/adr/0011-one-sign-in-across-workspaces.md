# 11. One sign-in across workspaces, with an account above them

Date: 2026-10-23. Status: accepted.

## Context

Phase 2 ends when Gannon runs Punchlist and Groundwork from Punchlist with no workaround outside it. ADR 0010 gives each workspace one repository, so the two projects are two workspaces. It says moving between them is a workspace switch in the web app, and that the inbox (R16) can span workspaces, but not how. On 2026-10-22 Gannon decided this needs one sign-in across several workspaces.

Today every identity is one row in one workspace:

- `actor.workspace_id` is `NOT NULL`, and a token is that row's `token_hash`.
- The `Actor` extractor (`crates/punchlist-server/src/auth.rs`) answers that one workspace. Handlers read it as `actor.workspace_id` at 94 sites in 11 server modules.
- The browser keeps the raw token in the one `punchlist_session` cookie (`src/session.rs`), so it is in one workspace at a time.
- `pl` reads one `server_url` and one `token` (`Config` in `crates/punchlist-cli/src/lib.rs`).
- A runner registers with a person's token and is a row of that workspace (`runner.workspace_id`). Its claims select from that workspace only.
- `punchlist-server bootstrap` is the only way to create a person with a token, and it always creates a new workspace with them.
- The Linear import creates each Linear user as a tokenless person in the workspace, so the dogfood workspace already has two Gannons: `Gannon` (bootstrap, with a token) and `Gannon Hall` (imported).

Five options:

- **(a) An account above workspaces, with the person actor as the membership.** A new `account` table holds the identity and the token hash. `actor.account_id` links a person actor to it, unique per `(workspace_id, account_id)`. The person actor stays the membership, so transitions, comments, `by` rules, owners of agents and every `actor.workspace_id` read stay as they are. The extractor finds the account by token, then the account's person actor in the workspace the request names. Cost: one migration, the extractor, sign-in, `GET /api/session`, `bootstrap`, a `GET /api/workspaces` route, and a way to name the workspace on workspace-level routes. The handlers do not change.
- **(b) A person per workspace, and a session that holds several tokens.** No schema change. The browser keeps one cookie per workspace (`punchlist_session_<workspace id>`), and the extractor picks the one for the request's workspace. Cost: the person pastes a token per workspace, so it is one sign-in per workspace, not one sign-in. The cookies carry several long-lived raw tokens. Nothing on the server knows that two persons are the same human, so an inbox across workspaces must read whichever cookies a request carries, and `pl` cannot span workspaces without a profile per token.
- **(c) A person actor with no workspace.** Make `actor.workspace_id` nullable for persons. Cost: each of the 94 `actor.workspace_id` reads, `list_actors`, `create_agent`'s owner lookup by name, the Linear import's per-workspace `linear_id` index, and the `transition` and `event` rows that name the actor need a membership check of their own. It is the largest change and gains nothing over (a).
- **(d) One instance per workspace.** Run a second server for Groundwork. No code. Cost: cookies ignore the port, so the two instances need two hostnames. Two sign-ins, two backups, two upgrades, and the inbox cannot span them. It is the workaround Phase 2 rules out.
- **(e) One workspace with two repositories.** It contradicts ADR 0010 and needs a home for the workflow that no single repository owns. Out of scope.

## Decision

- **The option: (a), an account above workspaces.** The person actor in each workspace is the account's membership there. It meets one sign-in, gives the inbox an identity to group by, and leaves the handlers alone.
- **What a token authenticates: a person's token authenticates the account.** Agent and runner tokens stay bound to one workspace, as today. Browser sign-in (`POST /api/session`) still takes a person's token only.
- **The cookie: unchanged.** Still `punchlist_session`, `HttpOnly; SameSite=Strict; Path=/`, and `Secure` over https, holding the account's token. Signing out clears it for every workspace.
- **How a request names its workspace: by its path.** `/api/workspaces/{id}/...` names it, as the event stream and imports already do. `/api/issues/{id}/...` names it by the issue, since issue ids are unique on the instance (`issue_prefix` is `UNIQUE`). `/api/runs/{id}`, `/api/proof/{id}` and `/api/projects/{id}/milestones` name it by their row. The workspace-level routes a person calls without one (`GET` and `POST /api/issues`, `/api/projects`, `/api/workflow`, `/api/actors`, `/api/runners`, `/api/pull-requests/unlinked`) gain a form under `/api/workspaces/{id}/`. The old form answers for the account's only workspace, else 400 naming the account's workspace prefixes, so a one-workspace `pl` keeps working. A workspace the account is not in is 404 on workspace routes and 403 on issue routes, as today.
- **The event stream: one per workspace, unchanged.** `GET /api/workspaces/{id}/events` (ADR 0006), since sequence numbers are per workspace. The web app streams the workspace it shows and switches streams when the person switches workspaces.
- **The web app: the URL names the workspace.** The list is at a path such as `/w/GW`, and `/` goes to the last workspace used, else the oldest. The issue page `/issues/GW-3` takes its workspace from the issue. Cached queries are keyed by workspace.
- **The runner: one runner process per workspace.** Runner rows, runner tokens, leases, claims and re-adoption by name are already scoped to one workspace, so the runner changes only to name the workspace at registration. Two runners on one machine do not collide: worktrees are keyed by repository owner and name (`crates/punchlist-runner/src/worktree.rs`), the MCP config and Codex home by run id, and the restart cleanup by runner id (`PUNCHLIST_RUNNER`). One runner across workspaces is a later phase.
- **`pl`: one token and a global `--workspace <prefix or id>`.** Also read from `PUNCHLIST_WORKSPACE` and the config key `workspace`. Issue commands take the workspace from the issue id and need no flag. No profiles, since one token reaches every workspace. The `--workspace` of `pl import linear` and `pl import status` becomes this flag.
- **Joining a workspace: `punchlist-server bootstrap --account <id>`.** It creates a workspace whose first person belongs to an existing account, and prints no new token. Inviting another person to a workspace stays out of scope.
- **The inbox (R16): grouped by account.** One inbox route reads every workspace the account belongs to. Whether it stays live through a stream per workspace or one merged stream is R16's decision.
- **The migration: one account per person actor with a token.** The account is named as the actor and takes its `token_hash`. The actor's `token_hash` becomes NULL and its `account_id` is set, and the `actor_github_identity` constraint is changed to allow that. Existing tokens and signed-in browsers keep working. Tokenless imported persons get no account. No issue, event, transition or run row changes.

## Consequences

- One sign-in reaches Punchlist and Groundwork, and moving between them is a URL change in the web app.
- Everything a workspace records still names a person actor of that workspace, so roles, locks, gates, `by` rules and the event log are unchanged.
- Every person request does one more lookup: from the account to its person actor in the named workspace.
- Each runner process has its own `--max-concurrent`, so a machine's agents are budgeted per workspace, not per machine.
- The imported `Gannon Hall` stays a separate tokenless person. Merging an imported person into an account's person actor is not decided here.
- Teams with several repositories in one workspace still wait for the ADR that lifts ADR 0010's limit.
