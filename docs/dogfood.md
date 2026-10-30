# Dogfood checklist

Gate 1 passes when Punchlist's own issues run from Punchlist for 14 consecutive days, with no fallback to Linear. Groundwork runs from the same instance since 2026-10-25 (PL-89), as a second workspace under Gannon's one sign-in. This page says how the instance on sartre is set up, what a day looks like, what to record, and what to do when the server is down.

## The instance

| | |
| --- | --- |
| Checkout | `~/dev/punchlist` on sartre, on `main`. `scripts/dogfood.sh` names the Compose project `punchlist-host`, so `pnpm dev` and `pnpm check` in the same checkout (project `punchlist`) never reach the instance |
| App | `https://sartre.tail984796.ts.net:7880`, tailnet only: `tailscale serve` in front of the server on `127.0.0.1:7880` (sartre's firewall drops other ports from the tailnet) |
| Postgres | `127.0.0.1:5440` on sartre, volume `punchlist-host_db` |
| Proof files | volume `punchlist-host_proof` |
| Webhooks | `https://sartre.tail984796.ts.net:10000/api/github/webhook`, a Tailscale Funnel on that one path to the app |
| GitHub App | `punchlist-sartre` (id 5416434), installed on `gannonh/punchlist` and `gannonh/groundwork`. Runs act on GitHub as it: their pushes, pull requests, comments, thread replies and merges show as `punchlist-sartre[bot]`. It needs Contents and Pull requests: Read and write, Checks: Read |
| Workspaces | `Punchlist` (prefix `PL`, `gannonh/punchlist`) and `Groundwork` (prefix `GW`, `gannonh/groundwork`, its workflow in that repository's `.punchlist/`) |
| `pl` config | `~/.config/punchlist/config.toml` (Gannon's account; `pl --workspace PL` names the workspace), `~/.config/punchlist/orchestrator.toml` (the orchestrator) |

Sign in with GitHub needs the App's OAuth client, which Gannon sets up once:

1. GitHub, Settings, Developer settings, GitHub Apps, `punchlist-sartre`, Edit.
2. Make `https://sartre.tail984796.ts.net:7880/api/auth/github/callback` the first Callback URL. The server sends GitHub no `redirect_uri` (GitHub refused it, PL-171), so GitHub sends every sign-in and link to the App's first Callback URL.
3. Under Client secrets choose Generate a new client secret and copy it.
4. In the Punchlist 1Password Environment add `DOGFOOD_GITHUB_APP_CLIENT_ID` (the App's Client ID) and `DOGFOOD_GITHUB_APP_CLIENT_SECRET`.
5. On sartre run `scripts/dogfood.sh up -d --build --wait`.

`scripts/dogfood.sh` sets `PUNCHLIST_PUBLIC_URL=https://sartre.tail984796.ts.net:7880`. Browsers must open the app at that URL: the server refuses a write whose `Origin` is another one. Without the two variables GitHub sign-in is off and the token field is the only way in. A person links GitHub once, from a token sign-in, with `Link GitHub` in the account menu. The upgrade that added sessions signs every browser out once; each signs in again.

To add a person, invite their GitHub login to each workspace they join. Their first sign-in with GitHub creates their account and a person in each workspace with an open invite; they need no token. A person who already has an account gets the new workspace at their next sign-in with GitHub.

```sh
pl --workspace PL invite create octo-cat   # Invited octo-cat: …
pl --workspace PL invite list              # open invites, with their ids
pl --workspace PL invite revoke <id>       # before they sign in
```

Start or update it:

```sh
cd ~/dev/punchlist && git pull --ff-only
scripts/dogfood.sh up -d --build --wait
SQLX_OFFLINE=true cargo install --locked --path crates/punchlist-cli   # pl, for the runner and the CLI
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:7880/api/openapi.json   # 200
```

`scripts/dogfood.sh` is `docker compose` with the instance's address and ports, and the App's id, private key and webhook secret from the Punchlist 1Password Environment, where they are `DOGFOOD_GITHUB_APP_ID`, `DOGFOOD_GITHUB_APP_PRIVATE_KEY` and `DOGFOOD_GITHUB_WEBHOOK_SECRET`; the plain `GITHUB_*` names there are the sandbox App's, for verify-punchlist. Start, stop and restart the stack through the script, since a plain `docker compose up` would recreate it on the default ports without the App. Plain `docker compose` in the checkout is the development project, so give it `-p punchlist-host` to reach the instance. The app for the tailnet, and the webhook Funnel for GitHub:

```sh
tailscale serve --bg --https=7880 http://127.0.0.1:7880
tailscale funnel --bg --https=10000 --set-path=/api/github/webhook \
  http://127.0.0.1:7880/api/github/webhook
```

The `pl` config on sartre has `server_url = "http://127.0.0.1:7880"`.

Each workspace has one runner, in the panes `runner` (Punchlist) and `runner-gw` (Groundwork) of the herdr workspace `punchlist-dogfood` on sartre. The instance itself runs in Docker, in no pane. Start the runners again after a reboot, one per pane:

```sh
pl --workspace PL runner start --name sartre-punchlist --agents claude-code,codex
pl --workspace GW runner start --name sartre-groundwork --agents claude-code,codex --max-concurrent 1
```

A runner serves the one workspace it starts in; both share the config and state directory. Groundwork's runner takes one issue at a time (`--max-concurrent 1`) until its runs' load on sartre is known: its live checks run Playwright and share one Postgres container.

The runner needs no `gh` login: each run gets an installation token of the App from the server, for its workspace's repository alone, and its agent sees no other GitHub credential. A run whose claim says the App cannot write the repository fails with that reason; accept the App's new permissions on the installation (GitHub, Settings, Applications, `punchlist-sartre`, Review request).

The runner works on one issue at a time unless told otherwise, whatever the workflow's `[dispatch] max_concurrent`, which caps the whole workspace. The `pl` config sets it to three:

```toml
[runner]
max_concurrent = 3
```

A runner keeps its name, so starting it again re-adopts the same runner. Stopping it with `Ctrl-C` (or SIGTERM) stops its agents and finishes each attempt `failed (runner stopped)`; the next claim starts attempt 2 of the same run on the same branch, with a new agent session. A runner that died without stopping them (a crash, `kill -9`, power loss) resumes its Claude Code attempts when it starts again within the lease (30 s), and so does one stopped with SIGHUP.

Upgrade a runner with SIGHUP, and start it again at once in its pane:

```sh
kill -HUP "$(pgrep -f 'runner start --name sartre-punchlist')"
pl --workspace PL runner start --name sartre-punchlist --agents claude-code,codex
```

SIGHUP stops each agent as `Ctrl-C` does but reports none of its attempts, and the runner exits 0 with `stopped for a restart; N attempt(s) left to resume`. The next start resumes each Claude Code attempt whose worktree is still on its branch in its session (`claude --resume`), and reports any other, Codex included, lost (`runner restarted`), so its run gets attempt 2. A runner not started again within 30 s leaves its leases to lapse, and the sweep fails the attempts, so each run's next attempt goes to any runner. A stack restart needs nothing from the runners: they warn while the server is down and heartbeat again when it is back.

## A day

Two people work the instance: Gannon, and one long-lived Claude Code session, the orchestrator, in the herdr workspace `punchlist-dogfood` on sartre (pane `orchestrator`), started in `~/dev/punchlist`. The orchestrator plans and grooms issues and reads the runs; it never implements an issue itself, so every implementing pull request comes from a runner dispatch. Its own pull requests (docs, prompts) are made in a worktree, never on the instance's checkout, and count against the 80% target.

The orchestrator acts in Punchlist as itself, the agent `Orchestrator`, so the timeline shows its issues, edits and comments as `Orchestrator (agent, claude-code)`, not as Gannon. It runs `pl` with `PUNCHLIST_CONFIG=~/.config/punchlist/orchestrator.toml`, which holds that agent's token. Its token creates, lists, edits and comments on issues, and reads the event stream, projects, the workflow, actors and proof files. It can move an issue to Todo or Canceled, and cannot make Gannon's moves (`todo → start`, `human_review → merging`), act in a status locked to agents, create projects or milestones, mint actors, or register runners. Mint the agent once, as Gannon:

```sh
pl --workspace PL actor create --agent --kind claude-code --name Orchestrator
# Prints the agent's token once. Write it into the orchestrator's config:
printf 'server_url = "http://127.0.0.1:7880"\ntoken = "%s"\n' '<token>' > ~/.config/punchlist/orchestrator.toml
chmod 600 ~/.config/punchlist/orchestrator.toml
```

Its GitHub actions (merges, pull request comments, `@coderabbitai` requests through `gh`) still show the account `gh` is logged in as.

1. Open the app. Read the board: what moved overnight, what waits on you in Human Review, and anything that shows `waiting on a person`.
2. Review each Human Review pull request on GitHub. Move it to Merging, or back with a comment. Merge from Merging only.
3. Before an issue leaves Backlog, the orchestrator grooms it: its blockers are Done, its file pointers match `main`, and it folds in what earlier runs taught. An issue whose slice changes a screen (its live checks number 10) gets the label `ui`, so `proof_attached` needs a screenshot and a video from its run before Human Review. Once it is ready the orchestrator moves it to Todo itself. Todo holds only issues that can run at the same time: no blocker short of Done, and no two that change the same code or must land in order; the orchestrator moves one back to Backlog, with a `Blocked by` line, when that stops holding. Agents may make any move except into Start and into Merging: those two are Gannon's, the start signal and the approval to land.
4. Move the next issues to Start. The runner claims them. Do not type a prompt into an agent for an issue; if an issue needs one, the issue body or the workflow's prompt is wrong, so fix that.
5. The orchestrator watches the workspace's event stream (`GET /api/workspaces/{id}/events`) and reports claims, moves, failed runs and refusals in its pane, so nobody has to announce a Start.
6. After each merge, the orchestrator reviews the issue's runs (attempts, lost or re-dispatched runs, duration and tokens, refused transitions, review threads and CI fixes, anything a person had to do) and files each product gap as a `PL` issue, or opens a pull request for a prompt gap in `.punchlist/prompts/`.
7. New work starts as an issue in Punchlist (`pl issue create` or the app), never in Linear.
8. At the end of the day, add a line to the gate check issue: the date, issues moved to Done, and anything that went wrong. A day that needed Linear, or an agent typed into by hand, ends the streak; record it and start the count again.

## The evidence

Run these against the instance's database (`psql postgres://punchlist:punchlist@127.0.0.1:5440/punchlist`) and paste the output into the gate check issue at the end of the 14 days.

Issues run, per workspace:

```sql
SELECT w.name, count(DISTINCT r.issue_id) FROM run r JOIN workspace w ON w.id = r.workspace_id GROUP BY w.name;
```

Merged pull requests, and how many came from a runner's branch (the target is 80% or more):

```sql
SELECT w.name, count(*) AS merged,
       count(*) FILTER (WHERE EXISTS (SELECT 1 FROM run r WHERE r.issue_id = p.issue_id AND r.branch = p.branch)) AS from_runner
FROM pull_request p JOIN issue i ON i.id = p.issue_id JOIN workspace w ON w.id = i.workspace_id
WHERE p.state = 'merged' GROUP BY w.name;
```

Agent moves out of a status locked to agents (the target is 0):

```sql
SELECT t.issue_id, t.from_status, t.to_status, a.name, t.created_at
FROM transition t JOIN actor a ON a.id = t.actor_id
WHERE a.role IN ('agent', 'runner') AND t.from_status = 'human_review';
```

Lost attempts, each with the attempt of the same run that replaced it (none when the run ended), and attempts on one issue that overlapped (the target for overlaps is 0):

```sql
SELECT a.issue_id, a.run_id, a.attempt, a.failure_reason, b.attempt AS replaced_by
FROM attempt a LEFT JOIN attempt b ON b.run_id = a.run_id AND b.attempt = a.attempt + 1
WHERE a.failure_reason ~ 'lost|lease expired|runner restarted|runner stopped' ORDER BY a.started_at;

SELECT a.issue_id, a.id, b.id FROM attempt a JOIN attempt b
  ON a.issue_id = b.issue_id AND a.id < b.id
 AND tstzrange(a.started_at, a.finished_at) && tstzrange(b.started_at, b.finished_at);
```

## When the server is down

There is no fallback to Linear: a day spent in Linear ends the streak. Get the server back instead.

1. Look: `cd ~/dev/punchlist && docker compose -p punchlist-host ps && docker compose -p punchlist-host logs --tail 100 server`.
2. Restart: `cd ~/dev/punchlist && scripts/dogfood.sh up -d --wait`. Migrations run at start; a failed migration is in the log.
3. If the data is lost or damaged, restore the last backup (below), then start the stack as in step 2. Pushes to `.punchlist/` that the server missed load at start. Pull requests that changed while it was down are read again before their next gated move.
4. Record the outage, and what it cost, in the gate check issue.

## Backups

Every night at 03:00, from a systemd user timer on sartre (Omarchy has no cron; linger is on, so it runs while logged out, and `Persistent=true` runs a missed night at the next boot). `~/.config/systemd/user/punchlist-backup.service`:

```ini
[Unit]
Description=Back up the Punchlist dogfood instance (docs/dogfood.md)

[Service]
Type=oneshot
WorkingDirectory=%h/dev/punchlist
ExecStart=/usr/bin/bash -c 'd=%h/backups/punchlist/$(date +%%F) && mkdir -p $d && docker compose -p punchlist-host exec -T db pg_dump -U punchlist -Fc punchlist > $d/db.dump && docker compose -p punchlist-host exec -T server tar -C /var/lib/punchlist -cf - proof > $d/proof.tar'
```

`~/.config/systemd/user/punchlist-backup.timer`:

```ini
[Unit]
Description=Nightly backup of the Punchlist dogfood instance

[Timer]
OnCalendar=*-*-* 03:00:00
Persistent=true

[Install]
WantedBy=timers.target
```

Turn it on with `systemctl --user daemon-reload && systemctl --user enable --now punchlist-backup.timer`. Back up now with `systemctl --user start punchlist-backup.service`; `systemctl --user list-timers` shows the next run.

Restore one. First stop the runner (`Ctrl-C` in its pane), so it cannot write run or lease state newer than the backup into the restored database; it stops its agents as it exits. Start it again as above once the stack is back.

```sh
cd ~/dev/punchlist && d=~/backups/punchlist/<date>
scripts/dogfood.sh stop server
scripts/dogfood.sh up -d --wait db
docker compose -p punchlist-host exec -T db pg_restore -U punchlist -d punchlist --clean --if-exists < $d/db.dump
docker compose -p punchlist-host run --rm -T --no-deps --entrypoint sh server -c 'rm -rf /var/lib/punchlist/proof/* && tar -C /var/lib/punchlist -xf -' < $d/proof.tar
```

Then start the stack as above.

Once a week, check that last night's backup restores, into a scratch project beside the instance:

```sh
cd ~/dev/punchlist && d=~/backups/punchlist/$(date +%F)
export COMPOSE_PROJECT_NAME=punchlist-restore PUNCHLIST_HOST=127.0.0.1 PUNCHLIST_PORT=7881 PUNCHLIST_DB_PORT=5441
docker compose up -d --wait db
docker compose exec -T db pg_restore -U punchlist -d punchlist --clean --if-exists < $d/db.dump
docker compose run --rm -T --no-deps --entrypoint sh server -c 'rm -rf /var/lib/punchlist/proof/* && tar -C /var/lib/punchlist -xf -' < $d/proof.tar
docker compose up -d --wait
q="SELECT (SELECT count(*) FROM issue), (SELECT count(*) FROM event), (SELECT count(*) FROM proof)"
docker compose exec -T db psql -U punchlist -tAc "$q"                                                       # the restore
docker compose -p punchlist-host exec -T db psql -U punchlist -tAc "$q"                                     # the instance; equal unless it changed since 03:00
docker compose exec -T server sh -c 'ls /var/lib/punchlist/proof | grep -vc part'                           # proof files: the restore's proof count
docker compose down -v && unset COMPOSE_PROJECT_NAME PUNCHLIST_HOST PUNCHLIST_PORT PUNCHLIST_DB_PORT
```
