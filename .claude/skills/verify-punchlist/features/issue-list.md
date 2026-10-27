# List issues

`pl issue list` prints every issue in the workspace, oldest first, one per line as `<id>  <Status>  <title>` with aligned columns, or `No issues.` when there are none.

## Sub-features

- `list-empty` prints `No issues.`.
- `list-rows` prints one aligned row per issue, ordered by number.
- `list-web` (web app only): the app's list and board filter the same rows by text, assignee, project, milestone, labels and statuses; [web-app.md](web-app.md) `web-filter` covers them. `pl issue list` has no filters.
- `list-bulk` (web app only): select rows on the list and set their assignee, labels, project or milestone from the right-click menu or a key; [web-app.md](web-app.md) `web-select` and `web-bulk-menu`. Each changed issue gets its own `PATCH` and `edited` event, so `SELECT issue_id, edited_fields FROM event WHERE kind = 'edited'` has one row per issue changed.
- `list-search`: `pl search <query>` (`GET /api/search?q=`) finds issues by id, title, body and comments, one literal phrase in any case; it prints `PL-12  <title>` per hit and, under a body or comment match, `    body: <before>[<hit>]<after>` or `    comment: …`. `No matches.` for none; `… more matches; narrow the query` past `--limit` (default 50, at most 100). Under 3 characters it prints `error: search needs at least 3 characters (422 Unprocessable Entity)` and exits 1. Snippets carry no control characters; a control character in a title prints as a space.
- `list-seq` (API only) returns `last_event_seq`, the workspace's latest event number of any kind (`created`, `edited`, `transition`, `comment`, `refusal`, `run`, `proof`, `pull_request`, `workflow`), read in the same snapshot as the list.

## How to get to it (user POV)

- `pl issue list`
- `GET /api/issues`

## Driving it with control-punchlist

Preconditions:

- Baseline run `$RUN`.

- **Empty.** Run `$C pl "$RUN" issue list`. Exit `0`, stdout `No issues.`.
- **Rows.** Create `First` and `Second`, move `PL-1` to `todo` and `start`, then run `$C pl "$RUN" issue list`. Stdout:

  ```text
  PL-1  Start    First
  PL-2  Backlog  Second
  ```

- **Sequence.** `source $TMPDIR/punchlist-verify/$RUN/run.env` and `$C curl "$RUN" "$SERVER_URL/api/issues"`. JSON has `"last_event_seq":4` (two `created` events and two `transition` events).

- **Search.** Create `Gate the transition` with body `The runner restarts and leaks ci_green on every retry.`, `Reword ci_green` with body `Plain.` and `Docs pass` with a comment `We saw 100% of retries pass.`. `$C pl "$RUN" search ci_green` prints:

  ```text
  PL-2  Reword ci_green
  PL-1  Gate the transition
      body: The runner restarts and leaks [ci_green] on every retry.
  ```

  `$C pl "$RUN" search 100%` prints `PL-3  Docs pass` and `    comment: We saw [100%] of retries pass.`; `$C curl "$RUN" "$SERVER_URL/api/search?q=100%25" | jq .hits[0].snippet.comment_seq` equals `$C sql "$RUN" "SELECT e.seq FROM event e JOIN comment c ON c.id = e.comment_id WHERE c.body LIKE '%100\%%'"`. An agent's token gets 403.

## Gotchas

- Column widths follow the longest id and status name, so the spacing changes with the data. `pl search` does not pad.
- `%` in a query sent with curl is `%25`; `q=100%` is a malformed query string.
