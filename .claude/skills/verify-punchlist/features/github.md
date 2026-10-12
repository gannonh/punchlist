# GitHub webhook, pull requests and checks

The server takes GitHub App webhook deliveries at `POST /api/github/webhook`, checks the `X-Hub-Signature-256` HMAC against its webhook secret, and queues each delivery as a `job` keyed by its `X-GitHub-Delivery` id. A background worker records the pull request, check run and review thread events as evidence, links a pull request to the issue its branch or title names, and moves that issue when the workflow's `on` rules say so. `pl issue show` prints the issue's pull requests and any comments; `pl pr unlinked` lists pull requests that name no issue.

## Sub-features

- `webhook-signature` answers `401` to a missing or wrong signature and queues nothing.
- `webhook-queue` answers `2xx` to a valid delivery and inserts one `job` row with `kind = github_event`.
- `pull-request-link` records a `pull_request` row whose `issue_id` is the issue named by the branch (`feature/pl-7-...`) or `(PL-7)` in the title; the branch wins when both name an issue.
- `pull-request-show` prints `#<n> <draft|open|closed|merged>  <branch>  checks <state> <passed>/<total>  <k> open threads` under `Pull requests` in `pl issue show`, then the URL.
- `checks` records `check_run` rows, each with the slug of the GitHub App that created it in `app` (`github-actions` for a workflow job; from the delivery's or the list's `app.slug`, NULL for a row recorded before the column), and sets `pull_request.checks` and the counts from the runs on the head commit.
- `threads` records `review_thread` rows, each with the `path` and `line` it is on (from GraphQL, or the event's first comment; the line it was on once the file changed under it), and sets `pull_request.open_threads` to the unresolved count. With App credentials a thread event makes the server read every review thread of the pull request from GitHub and replace the recorded ones, so the rows are GitHub's state now, whatever order the events came in, and include threads no event named. Each row also holds the thread's `comments` (a JSON array of `{author, body, cut}`: the last 10, each body at most 2000 characters, `cut` true when it was longer) and `comments_total`; an event without comments keeps what is recorded. A pull request with more than 1000 review threads fails the read. `POST /api/issues/{id}/pull-requests/refresh` reads GitHub first, as a gated request does, and answers `{pull_requests, review_threads, refreshed}`: the unresolved threads of the issue's open pull requests, and `refreshed: false` with what is recorded when GitHub cannot be read or the run has no App credentials. `GET /api/issues/{id}/pull-requests` returns no thread text.
- `close-read` checks a payload that says closed without a merge against GitHub, with App credentials, and records what GitHub says now: a `closed` delivered after the `reopened` of the same second leaves the pull request open and the issue where it is.
- `read-order` keeps an older read of a pull request from replacing a newer one. `pull_request.version` counts the reads recorded; a job or a gated request notes it before it reads GitHub and records only if it is unchanged. A job whose read is dropped reads again in the same attempt, up to three reads, and logs `another read of the pull request was recorded first, reading again`; a gated request reads the dropped pull request once more and logs `another read of the pull request was recorded first, this one is dropped`.
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
- **Threads.** Deliver a `pull_request_review_thread` event with action `unresolved`, then `$C sql "$RUN" "SELECT node_id, resolved FROM review_thread"` has one unresolved row and `pl issue show PL-1` prints `1 open thread`. Deliver `resolved` for the same `thread.node_id` and it prints `0 open threads`. GitHub sends this event only on resolve and unresolve, so a thread nobody has resolved yet is not counted in a run without App credentials, which is what these fixture deliveries assume: with them the server asks the sandbox for pull request 7's threads instead, and the job fails if there is no such pull request.
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

Then open a branch named `feature/pl-1-...` and a pull request in the sandbox repository (`gh api` to create the ref and a file, `gh pr create -R gannonh/punchlist-sandbox --draft`), and read the same rows as above. The sandbox's `ci` workflow gives every pull request one check run named `ok`. It fails on demand: on a branch with a file named `ci-fail` at the repository root. Delete the file on the branch and the new commit's `ok` passes. A second workflow, `ci2.yml`, also has a job named `ok` (another check suite) and fails on a file named `ci2-fail` at the root; a pull request therefore has two `ok` check runs, and `SELECT name, check_suite_id, conclusion FROM check_run` shows both. A rerun of a run (`gh run rerun`) is a new run id in the same suite.

GitHub's delivery log is the first place to look when nothing arrives, and redelivery is how a check repeats a delivery (the same as the Redeliver button on the App's Advanced page):

```sh
A=.claude/skills/verify-punchlist/bin/github-app
with-env $A GET '/app/hook/deliveries?per_page=10'       # delivered_at, event, action, guid, status_code, status
with-env $A POST /app/hook/deliveries/<id>/attempts      # redeliver; the guid stays the same
```

A redelivery keeps its `guid`, so `SELECT count(*) FROM job WHERE idempotency_key = '<guid>'` stays `1`. GitHub's `delivered_at` is stamped after the server answers, so a job's `finished_at` can be a few milliseconds earlier. Do not paste the secret or the delivery payloads' tokens into evidence.

### Deliveries out of order

A run under `with-env` reads GitHub when a delivery could be stale, so these need a real sandbox pull request, linked to an issue by its branch. To make a delivery late, stop only the server (`down` drops the database): `kill -TERM -- -$(cat /tmp/punchlist-verify/$RUN/server.pgid)`. Act on GitHub, then start it again under `with-env` with the run's `DATABASE_URL`, `PUNCHLIST_BIND=127.0.0.1:7879` and `GITHUB_APP_ID=5254938`, as `setsid pnpm dev` appending to the run's `server.log`, and write the new process group id to `server.pgid` so `down` still stops it. A delivery that got `502` while the server was down left no `job` row, so its redelivery is processed.

- **Closed after reopened.** A `closed` payload for a pull request reopened within the second already says open, so GitHub cannot be made to send the stale pair; replay it. With the server stopped, close and reopen the pull request (two `gh api -X PATCH … -f state=…` calls), start the server and redeliver `reopened`. Take the real `closed` payload (`with-env $A GET /app/hook/deliveries/<id>`, `request.payload`; it says `"state": "closed"`), set `pull_request.updated_at` to the recorded `github_updated_at`, and deliver it with `$C webhook "$RUN" pull_request <file> <delivery-id>`, which signs with the App's secret in a `with-env` run. `pl issue show` lists the pull request `open`, the issue has not moved, and `SELECT count(*) FROM transition WHERE issue_id = '<id>' AND from_status = 'in_progress' AND to_status = 'todo'` is `0`. Before this read the same replay recorded it closed and moved the issue to Todo.
- **Resolved after unresolved.** Add a review comment on a diff line (`gh api repos/gannonh/punchlist-sandbox/pulls/<n>/comments -f body=... -f commit_id=<head sha> -f path=<file> -F line=1`), find the thread's id with GraphQL `reviewThreads { nodes { id isResolved } }`, and with the server stopped run the `resolveReviewThread` then `unresolveReviewThread` mutations. Start the server, redeliver `unresolved`, then `resolved`. `SELECT node_id, resolved FROM review_thread` shows the thread with `f`, and `pl issue show` prints `1 open thread`.
- **A thread no event named.** Add a second review comment and resolve the first thread. `review_thread` has two rows and `pl issue show` prints `1 open thread`: the new one.
- **A read held while another is recorded.** Hold the answer to one review thread read (`POST /graphql`) between GitHub and the server. With a thread recorded resolved: start holding, unresolve the thread (its job's read says unresolved and is held), resolve it, make a gated request, then let the read go, within the 15 seconds the job has to read. `review_thread` keeps `t` and `open_threads` stays `0`; the job's row is `done` with `attempts` 1 and no `last_error`, the server log has `reading again`, and `version` is one higher for the gated request and one higher for the job's second read.
- **A gated request without a model call.** Register a runner and claim through the API: `POST /api/runners` with `{"name": "r", "agents": ["claude-code"]}` as the person, then `POST /api/runs/claim` with `{}` as the runner's `token`; the answer's `agent_token` makes `POST /api/issues/<id>/transitions` requests until the attempt's 30-second lease lapses. Use an issue no old sandbox pull request names (`PL-9`, after eight fillers): a gated request reads every recent pull request that names its issue, three requests each, and dozens of closed sandbox pull requests name `PL-1`.

## Gotchas

- Out-of-order protection needs App credentials. A run started without `with-env` logs `no GitHub App credentials` and records payloads in arrival order, as before.
- `pull_request.version` goes up by one for each read recorded (a thread event's job, a close read, each pull request of a gated request's refresh) and stays put for a dropped read, for a payload recorded without a read and for a read GitHub answered 404 or 410, so a run without App credentials leaves it at `0`.
- In a run with App credentials a fixture review thread delivery is answered from the sandbox's threads for that pull request number, not from the fixture; use a run without `with-env` for fixture deliveries. A job whose read of GitHub fails stays `queued` with the call in `job.last_error` and is retried.

- `502 failed to connect to host` in the delivery log means GitHub could not reach the Funnel. Right after the Funnel is first turned on, public DNS for it can take some minutes; redeliver once `https://sartre.tail984796.ts.net:8443/api/github/webhook` answers from outside the tailnet.
- CI's `check_run` deliveries can land after the `pull_request` event that a check is counting; read `job` rows by `idempotency_key` rather than comparing totals.

- `up` without `with-env` uses a random secret, so only `$C webhook` deliveries verify; a real GitHub delivery gets `401`.
- The worker is asynchronous: the `webhook` command's `2xx` means queued, not processed. Poll the `job` row until `done` before reading other tables.
- Replaying a delivery with a new id is a new delivery. Only the same `X-GitHub-Delivery` is a no-op.
- Delivery ids are free text in fixtures, but real ones are UUIDs; `$C webhook` makes a UUID when none is given.
- `$C sql` is read-only by convention: do not insert `pull_request` rows by hand. The CLI integration tests do, and they say so.
- `pl issue show` always prints the `Pull requests` section; with none it reads `None linked.`
