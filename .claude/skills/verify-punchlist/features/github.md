# GitHub webhook, pull requests and checks

The server takes GitHub App webhook deliveries at `POST /api/github/webhook`, checks the `X-Hub-Signature-256` HMAC against its webhook secret, and queues each delivery as a `job` keyed by its `X-GitHub-Delivery` id. A background worker records the pull request, check run and review thread events as evidence, links a pull request to the issue its branch or title names, and moves that issue when the workflow's `on` rules say so. `pl issue show` prints the issue's pull requests and any comments; `pl pr unlinked` lists pull requests that name no issue.

## Sub-features

- `webhook-signature` answers `401` to a missing or wrong signature and queues nothing.
- `webhook-queue` answers `2xx` to a valid delivery and inserts one `job` row with `kind = github_event`.
- `pull-request-link` records a `pull_request` row whose `issue_id` is the issue named by the branch (`feature/pl-7-...`) or `(PL-7)` in the title; the branch wins when both name an issue.
- `pull-request-show` prints `#<n> <draft|open|closed|merged>  <branch>  checks <state> <passed>/<total>  <k> open threads` under `Pull requests` in `pl issue show`, then the URL.
- `checks` records `check_run` rows and sets `pull_request.checks` and the counts from the runs on the head commit.
- `threads` records `review_thread` rows and sets `pull_request.open_threads` to the unresolved count.
- `github-move` moves the issue as actor role `github` and records the delivery id on the `transition`; the timeline line ends `[delivery <id>]`.
- `comment` records a `comment` row and an `event` of kind `comment`; the timeline shows `<actor> (github)  commented:` and the body indented.
- `unlinked` lists a pull request that names no issue under `pl pr unlinked` and not on any issue.
- `redelivery` answers the same delivery id again without a second `job` row or a second effect.

## How to get to it (user POV)

- Open, update, review or close a pull request on the workspace's repository on GitHub, then `pl issue show <id>`.
- `pl pr unlinked`
- `GET /api/issues/{id}/pull-requests`; `GET /api/pull-requests/unlinked`.
- Maintainers of the checks: `$C webhook "$RUN" <event> <payload-file> [delivery-id]` plays the part of GitHub.

## Driving it with control-punchlist

Preconditions:

- Baseline run `$RUN` from `$C up` (it gave the server a webhook secret). `PL-1` exists (`$C pl "$RUN" issue create --title "Fix"`), and a runner has moved it to In Progress or the check says otherwise.
- Fixture payloads are JSON files shaped like GitHub's events, for the repository `gannonh/punchlist-sandbox`. Start from the server's test fixtures in `crates/punchlist-server/tests/fixtures/github/` and change `repository.owner.login`, `repository.name`, the branch and the number. Write them under `verify-artifacts/$RUN/` so they stay with the evidence.

- **Bad signature.** Run `curl -s -o /dev/null -w '%{http_code}' -X POST -H 'X-GitHub-Event: ping' -H 'X-GitHub-Delivery: bad-1' -H 'X-Hub-Signature-256: sha256=00' -d '{}' "$SERVER_URL/api/github/webhook"`. Prints `401`. `$C sql "$RUN" "SELECT count(*) FROM job"` shows `0`.
- **Deliver a pull request.** Save a `pull_request` event with `"action":"opened"`, `number` 7, `head.ref` `feature/pl-1-fix`, `draft` true to `$EVIDENCE/pr-opened.json`, then run `$C webhook "$RUN" pull_request "$EVIDENCE/pr-opened.json" delivery-1`. Prints `HTTP 2xx`.
- **Job.** Run `$C sql "$RUN" "SELECT kind, idempotency_key, state FROM job"`. One row `github_event | delivery-1 | done` once the worker has run (poll for a few seconds; `queued` or `running` first is fine).
- **Rows.** Run `$C sql "$RUN" "SELECT number, branch, state, draft, issue_id, checks, open_threads FROM pull_request"`. One row `7 | feature/pl-1-fix | open | t | PL-1 | none | 0`.
- **Show.** Run `$C pl "$RUN" issue show PL-1`. Stdout contains `#7 draft  feature/pl-1-fix  no checks  0 open threads`, and the next line is the pull request URL indented four spaces.
- **Checks.** Deliver a `check_run` event (`completed`, `success`) for the pull request's `head_sha`, then `$C sql "$RUN" "SELECT name, status, conclusion FROM check_run"` has the run and `pl issue show PL-1` prints `checks passing 1/1`.
- **Threads.** Deliver a `pull_request_review_thread` event with action `unresolved`, then `$C sql "$RUN" "SELECT node_id, resolved FROM review_thread"` has one unresolved row and `pl issue show PL-1` prints `1 open thread`. Deliver `resolved` for the same `thread.node_id` and it prints `0 open threads`. GitHub sends this event only on resolve and unresolve, so a thread nobody has resolved yet is not counted.
- **Move and comment.** Deliver the events the workflow's `on` rules use, such as `closed` with `merged: true`. Then `$C sql "$RUN" "SELECT t.from_status, t.to_status, t.delivery_id, a.role FROM transition t JOIN actor a ON a.id = t.actor_id ORDER BY t.created_at DESC LIMIT 1"` shows role `github` and the delivery id, `pl issue show PL-1` ends the timeline line with `[delivery <id>]`, and `SELECT body FROM comment` shows any comment the rule required.
- **Unlinked.** Deliver a `pull_request` `opened` event for number 8 on branch `stray` with a title naming no issue. `$C pl "$RUN" pr unlinked` prints `#8  open  stray  <title>`, and `pl issue show PL-1` still lists only `#7`. With none, it prints `No unlinked pull requests.`
- **Redelivery.** Run the first `$C webhook` command again with the same delivery id `delivery-1`. Then `$C sql "$RUN" "SELECT count(*) FROM job WHERE idempotency_key = 'delivery-1'"` shows `1`, and the `pull_request`, `transition` and `comment` row counts are what they were before.

## Real GitHub deliveries

Fixture deliveries prove the server's handling. Real events need more, and a check that depends on them is skipped, with the command tried, when any of this is missing:

- The Punchlist GitHub App is installed on `gannonh/punchlist-sandbox` (never on another repository).
- The App's webhook URL is `https://sartre.tail984796.ts.net:8443/api/github/webhook`. A Tailscale Funnel on sartre forwards that one path, and nothing else, to `http://127.0.0.1:7879/api/github/webhook`. Check it with `tailscale funnel status` (it lists `:8443 (Funnel on)`); turn it on again with `tailscale funnel --bg --https=8443 --set-path=/api/github/webhook http://127.0.0.1:7879/api/github/webhook`. With no server on 7879 the public URL answers 502.
- Only one run can receive real deliveries at a time: start it on the Funnel's port with `PUNCHLIST_VERIFY_PORT=7879 with-env $C up`. `up` refuses if 7879 is taken.
- The run was started under `with-env` (`with-env $C up`) so the server's webhook secret is the App's. Without it GitHub's signatures fail with `401`.

The App is **Punchlist (gannonh)** (`punchlist-gannonh`, App id 5254938), installed on `gannonh/punchlist-sandbox` only. The Punchlist 1Password Environment holds `GITHUB_WEBHOOK_SECRET` (the App's webhook secret) and `GITHUB_APP_PRIVATE_KEY` (on one line; `bin/github-app` rebuilds the PEM).

Then open a branch named `feature/pl-1-...` and a pull request in the sandbox repository (`gh api` to create the ref and a file, `gh pr create -R gannonh/punchlist-sandbox --draft`), and read the same rows as above. The sandbox's `ci` workflow gives every pull request one check run named `ok`.

GitHub's delivery log is the first place to look when nothing arrives, and redelivery is how a check repeats a delivery (the same as the Redeliver button on the App's Advanced page):

```sh
A=.claude/skills/verify-punchlist/bin/github-app
with-env $A GET '/app/hook/deliveries?per_page=10'       # delivered_at, event, action, guid, status_code, status
with-env $A POST /app/hook/deliveries/<id>/attempts      # redeliver; the guid stays the same
```

A redelivery keeps its `guid`, so `SELECT count(*) FROM job WHERE idempotency_key = '<guid>'` stays `1`. GitHub's `delivered_at` is stamped after the server answers, so a job's `finished_at` can be a few milliseconds earlier. Do not paste the secret or the delivery payloads' tokens into evidence.

## Gotchas

- `502 failed to connect to host` in the delivery log means GitHub could not reach the Funnel. Right after the Funnel is first turned on, public DNS for it can take some minutes; redeliver once `https://sartre.tail984796.ts.net:8443/api/github/webhook` answers from outside the tailnet.
- CI's `check_run` and `check_suite` deliveries can land after the `pull_request` event that a check is counting; read `job` rows by `idempotency_key` rather than comparing totals.

- `up` without `with-env` uses a random secret, so only `$C webhook` deliveries verify; a real GitHub delivery gets `401`.
- The worker is asynchronous: the `webhook` command's `2xx` means queued, not processed. Poll the `job` row until `done` before reading other tables.
- Replaying a delivery with a new id is a new delivery. Only the same `X-GitHub-Delivery` is a no-op.
- Delivery ids are free text in fixtures, but real ones are UUIDs; `$C webhook` makes a UUID when none is given.
- `$C sql` is read-only by convention: do not insert `pull_request` rows by hand. The CLI integration tests do, and they say so.
- `pl issue show` always prints the `Pull requests` section; with none it reads `None linked.`
