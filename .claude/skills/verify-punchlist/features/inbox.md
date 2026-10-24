# Inbox: the issues waiting on a person

A signed-in person's inbox lists the workspace's issues in a status only a person moves on (`Workflow::person_statuses()`: not the first status, no agent dispatched there, and every move a caller can request is one a person can make, at least one only a person) that are unassigned, assigned to them, or assigned to an agent they own. On the built-in workflow that is Todo, Human Review and Merging; on Punchlist's own, Todo and Human Review. An item leaves when its issue moves out of the status and has no dismiss. Opening one marks it read up to its `since_seq`, the issue's newest `transition` or `created` event, so a new move into a waiting status makes it unread again. The header of the list, the board, the Pull requests page and the inbox shows `Inbox` with the unread count, kept current by the event stream.

## Sub-features

- `inbox-api`: `GET /api/inbox` (and `/api/workspaces/{id}/inbox`) answers `{items, unread}`, each item `{kind: "waiting", issue: {id, title, status, status_name}, since, since_seq, unread}`, newest `since_seq` first. People only: an agent's or runner's token gets 403, no token 401.
- `inbox-read`: `POST /api/inbox/read` (and `/api/workspaces/{id}/inbox/read`), body `{issue, kind: "waiting", since_seq}`, answers 204 and keeps the larger `since_seq` in `inbox_read`; an issue outside the workspace answers 404. Same token rules.
- `inbox-header`: `[data-inbox-count]` in the header link `Inbox` (`/w/<prefix>/inbox`), read once `GET /api/issues` has succeeded, then again 100 ms after the last `transition`, `created`, `edited`, `run` or `workflow` event of a burst. `99+` above 99.
- `inbox-screen`: `/w/<prefix>/inbox` (and `/inbox`, which opens the workspace last used). A `Waiting on you` section (`[data-section=waiting]`) with one link per item, `[data-item=<id>]` (`data-unread` when unread), showing the id, `Waiting on you`, the status name (`[data-field=status]`), the age (`[data-age]`, such as `4m ago`) and the title. `Nothing waits on you.` (`[data-empty]`) with none.
- `inbox-keys`: `i` opens the inbox from the list, the board, the Pull requests page and the inbox (not in a field or with a dialog or menu open). `Enter` on an item (or a click) posts the read, then opens `/issues/<id>`. `Esc` goes to the list.
- `inbox-perf`: each commit of the screen with its data records `pl:render` with `detail: {layout: 'inbox', items}`.

## How to get to it (user POV)

- `$C curl "$RUN" "$SERVER_URL/api/inbox"` as the run's person.
- The app signed in (`$C sign-in-url "$RUN"`): the header's `Inbox` link, or `i`.

## Driving it with control-punchlist

Preconditions:

- A run started with `RUN=$($C up)` (the built-in workflow), `source /tmp/punchlist-verify/$RUN/run.env`, and `pnpm build` for the screen.

- **Setup.** `$C pl "$RUN" issue create --title One` four times (PL-1 to PL-4); `$C pl "$RUN" issue move PL-1 todo`; `$C sql "$RUN" "UPDATE issue SET status = 'human_review' WHERE id = 'PL-3'"` and `... 'merging' WHERE id = 'PL-4'`, since only an agent and GitHub reach them.
- **A second person.** Persons hold no token since accounts (ADR 0011); a second person is an account and its actor: `TOKEN=plt_second_$RANDOM; $C sql "$RUN" "WITH acc AS (INSERT INTO account (name, token_hash) VALUES ('Second', encode(sha256('$TOKEN'), 'hex')) RETURNING id) INSERT INTO actor (workspace_id, name, role, account_id) SELECT (SELECT workspace_id FROM actor WHERE role = 'person' LIMIT 1), 'Second', 'person', id FROM acc RETURNING id"`. Call as them with `curl -s -H "Authorization: Bearer $TOKEN" "$SERVER_URL/api/inbox"`.
- **The list.** `$C curl "$RUN" "$SERVER_URL/api/inbox" | jq -c '[.items[].issue.id], .unread'`. Pass on the setup when it prints `["PL-1","PL-4","PL-3"]` and `3`.
- **Audience.** `$C curl "$RUN" -X PATCH -H 'content-type: application/json' -d '{"assignee":"<second id>"}' "$SERVER_URL/api/issues/PL-1"`; an agent of the first person: `$C pl "$RUN" actor create --agent --kind claude-code` (prints its id and token), then the same PATCH with its id on PL-3.
- **Tokens.** `curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer <agent token>" "$SERVER_URL/api/inbox"` (403), the same with a runner token from `POST /api/runners` (403) and with no header (401).
- **Read.** `$C curl "$RUN" -X POST -H 'content-type: application/json' -d '{"issue":"PL-4","kind":"waiting","since_seq":<n>}' -w '%{http_code}' "$SERVER_URL/api/inbox/read"` prints `204`; `$C sql "$RUN" "SELECT actor_id, issue_id, since_seq FROM inbox_read"` has the row. `since_seq` is `SELECT max(seq) FROM event WHERE issue_id = 'PL-4' AND kind IN ('transition', 'created')`.
- **Live header.** With the list open, `$C pl "$RUN" issue move PL-2 todo`. Pass when `document.querySelector('[data-inbox-count]').textContent` rises by one within 2 s, and `data-last-seq` on the page root equals `SELECT max(seq) FROM event`.
- **Screen.** Press `i`: `location.pathname` is `/w/PL/inbox`; `[...document.querySelectorAll('[data-item]')].map(e => e.dataset.item)` is the API's order. Focus an item, press `Enter`: `/issues/<id>`, and the API's item has `unread` false. At 390 px, `document.documentElement.scrollWidth <= document.documentElement.clientWidth`.
- **Perf.** `for i in 1 2 3 4 5; do $C curl "$RUN" -o /dev/null -w '%{time_total}\n' "$SERVER_URL/api/inbox"; done`, and the network log (Playwright `page.on('request')`) holds one `GET /api/workspaces/<id>/inbox` about 100 ms after one move's stream message.
- **E2E.** `web/e2e/inbox.spec.ts`.

## Gotchas

- The setup writes rows (`UPDATE issue`, the second person) that `pl` cannot reach; the issue's live checks name them, so they are the exception to the read-only SQL rule.
- An issue put in its status by `UPDATE` keeps its `created` event as `since_seq`, so it sorts by creation, not by the update.
- Every unassigned issue is in every person's inbox, so a count in a shared run moves with other checks' issues; compare with the API's `unread` at the same moment.
- `GET /api/inbox` without a workspace in the path answers 400 `workspace_not_named` for an account in several workspaces; the app always calls `/api/workspaces/{id}/inbox`.
