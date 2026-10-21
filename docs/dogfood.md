# Dogfood checklist

Gate 1 passes when Punchlist's own issues run from Punchlist for 14 consecutive days, with no fallback to Linear. Groundwork joins once one sign-in can serve several workspaces. This page says how the instance on sartre is set up, what a day looks like, what to record, and what to do when the server is down.

## The instance

| | |
| --- | --- |
| Checkout | `~/srv/punchlist-host` on sartre, a clone of `gannonh/punchlist` on `main`. Its directory name makes the Compose project `punchlist-host`, apart from development checkouts |
| App | `http://100.96.231.20:7880` (sartre's Tailscale address) |
| Postgres | `127.0.0.1:5440` on sartre, volume `punchlist-host_db` |
| Proof files | volume `punchlist-host_proof` |
| Webhooks | `https://sartre.tail984796.ts.net:10000/api/github/webhook`, a Tailscale Funnel on that one path to the app |
| GitHub App | `punchlist-sartre` (id 5416434), installed on `gannonh/punchlist` only |
| Workspace | `Punchlist` (prefix `PL`, `gannonh/punchlist`) |
| `pl` config | `~/.config/punchlist/punchlist.toml` |

Start or update it:

```sh
cd ~/srv/punchlist-host && git pull --ff-only
scripts/dogfood.sh up -d --build --wait
SQLX_OFFLINE=true cargo install --locked --path crates/punchlist-cli   # pl, for the runner and the CLI
curl -s -o /dev/null -w '%{http_code}\n' http://100.96.231.20:7880/api/openapi.json   # 200
```

`scripts/dogfood.sh` is `docker compose` with the instance's address and ports, and the App's id, private key and webhook secret from the Punchlist 1Password Environment, where they are `DOGFOOD_GITHUB_APP_ID`, `DOGFOOD_GITHUB_APP_PRIVATE_KEY` and `DOGFOOD_GITHUB_WEBHOOK_SECRET`; the plain `GITHUB_*` names there are the sandbox App's, for verify-punchlist. Start, stop and restart the stack through the script, since a plain `docker compose up` would recreate it on the default ports without the App. `ps`, `logs` and `exec` work either way. The webhook Funnel:

```sh
tailscale funnel --bg --https=10000 --set-path=/api/github/webhook \
  http://100.96.231.20:7880/api/github/webhook
```

The workspace has one runner, in its own herdr pane on sartre. Start it again after a reboot:

```sh
PUNCHLIST_CONFIG=~/.config/punchlist/punchlist.toml pl runner start --name sartre-punchlist --agents claude-code,codex
```

A runner keeps its name, so starting it again re-adopts the same runner and resumes its Claude Code attempts. A stack restart needs nothing from the runners: they warn while the server is down and heartbeat again when it is back.

## A day

1. Open the app. Read the board: what moved overnight, what waits on you in Human Review, and anything that shows `waiting on a person`.
2. Review each Human Review pull request on GitHub. Move it to Merging, or back with a comment. Merge from Merging only.
3. Pick the next issues and move them to Start. The runner claims them. Do not type a prompt into an agent for an issue; if an issue needs one, the issue body or the workflow's prompt is wrong, so fix that.
4. New work starts as an issue in Punchlist (`pl issue create` or the app), never in Linear.
5. At the end of the day, add a line to the gate check issue: the date, issues moved to Done, and anything that went wrong. A day that needed Linear, or an agent typed into by hand, ends the streak; record it and start the count again.

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

1. Look: `cd ~/srv/punchlist-host && docker compose ps && docker compose logs --tail 100 server`.
2. Restart: `cd ~/srv/punchlist-host && scripts/dogfood.sh up -d --wait`. Migrations run at start; a failed migration is in the log.
3. If the data is lost or damaged, restore the last backup (below), then start the stack as in step 2. Pushes to `.punchlist/` that the server missed load at start. Pull requests that changed while it was down are read again before their next gated move.
4. Record the outage, and what it cost, in the gate check issue.

## Backups

Every night, from cron on sartre (`crontab -e`):

```sh
0 3 * * * cd ~/srv/punchlist-host && d=~/backups/punchlist/$(date +\%F) && mkdir -p $d && docker compose exec -T db pg_dump -U punchlist -Fc punchlist > $d/db.dump && docker compose exec -T server tar -C /var/lib/punchlist -cf - proof > $d/proof.tar
```

Restore one:

```sh
cd ~/srv/punchlist-host && d=~/backups/punchlist/<date>
scripts/dogfood.sh stop server
scripts/dogfood.sh up -d --wait db
docker compose exec -T db pg_restore -U punchlist -d punchlist --clean --if-exists < $d/db.dump
docker compose run --rm -T --no-deps --entrypoint sh server -c 'rm -rf /var/lib/punchlist/proof/* && tar -C /var/lib/punchlist -xf -' < $d/proof.tar
```

Then start the stack as above.

Once a week, check that last night's backup restores, into a scratch project beside the instance:

```sh
cd ~/srv/punchlist-host && d=~/backups/punchlist/$(date +%F)
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
