# PL-68 live checks

Branch `feature/pl-68`, run on sartre with `.claude/skills/verify-punchlist/bin/control-punchlist` (`PUNCHLIST_DB_PORT=5468`). Main is `origin/main` at `1242c5c` in its own worktree and run. Headless Chromium through the repository's Playwright. Scripts: `branch.mjs` (checks 1, 3 to 9), `perf.mjs` (checks 1 on main and 10), `seed.sh` (check 10's data).

Setup (run `pl68-branch`): `PL-1` to `PL-3` created with `pl issue create`; PL-1 and PL-2 moved to Todo and Start with `pl issue move`, then claimed by a runner through `POST /api/runs/claim` (both 200), so both are `in_progress`; PL-3 set to `merging` with `$C sql`. Pull requests delivered as signed `pull_request` webhooks from the fixtures, repository changed to `gannonh/punchlist-sandbox`: 7 `feature/pl-1-a` open, 8 `feature/pl-2-b` draft, 9 `stray` `Bump deps`, 14 `feature/pl-3-d` open.

```
 number | state | draft | merge_state | issue_id |                         url
--------+-------+-------+-------------+----------+------------------------------------------------------
      7 | open  | f     | unknown     | PL-1     | https://github.com/gannonh/punchlist-sandbox/pull/7
      8 | open  | t     | unknown     | PL-2     | https://github.com/gannonh/punchlist-sandbox/pull/8
      9 | open  | f     | unknown     |          | https://github.com/gannonh/punchlist-sandbox/pull/9
     14 | open  | f     | unknown     | PL-3     | https://github.com/gannonh/punchlist-sandbox/pull/14
```

| # | Check | Result |
| --- | --- | --- |
| 1 | Same flow on main and branch | PASS |
| 2 | Route contents | PASS |
| 3 | Entry and keys | PASS |
| 4 | Sections and rows | PASS |
| 5 | Waits on a merge | PASS |
| 6 | Live: appears and moves | PASS |
| 7 | Live: checks, threads | PASS |
| 8 | Unlinked | PASS, with a note on the comment event |
| 9 | Links and plain text | PASS |
| 10 | Render budget and request set | PASS |

## 1. Same flow on main and on the branch

Signed in, on `/` once the rows rendered:

- main: `{"link":false,"route":404}`. `document.querySelector('a[href="/pull-requests"]')` is null and `GET /api/pull-requests` answers 404.
- branch: `{"link":true,"route":200}`, and `/pull-requests` shows 4 `[data-section]` elements.

## 2. Route contents

PR 10 delivered first as `closed` with `merged: true`: `SELECT number, state FROM pull_request WHERE number = 10` is `10 | merged`.

```
$ $C curl "$RUN" "$SERVER_URL/api/pull-requests" | jq -c '[.[] | {number, draft, issue: .issue.id, waits_on_merge}]'
[{"number":14,"draft":false,"issue":"PL-3","waits_on_merge":true},{"number":9,"draft":false,"issue":null,"waits_on_merge":false},{"number":8,"draft":true,"issue":"PL-2","waits_on_merge":false},{"number":7,"draft":false,"issue":"PL-1","waits_on_merge":false}]
```

That string equals the expected one exactly, and 10 is absent. With a standalone agent's bearer token (`pl actor create --agent --kind codex`): `HTTP 403`, `{"code":"forbidden","message":"an agent acts only on the workspace's issues"}`.

## 3. Entry and keys

`{"list":{"path":"/pull-requests","count":"4"},"board":{"path":"/pull-requests","count":"4"},"chord":"/pull-requests","typed":{"path":"/","value":"gp"},"escape":"/"}`

## 4. Sections and rows

- Sections: `["waits-on-merge","in-review","draft","names-no-issue"]`.
- Row 7 text: `#7 Live one feature/pl-1-a PL-1 In Progress checks 0/0 0 open threads Merge state not known yet Open on GitHub`.
- `SELECT merge_state FROM pull_request WHERE number = 7` is `unknown`, and the label is `Merge state not known yet`.
- Row 9 contains `names no issue`.
- At 390 by 844, `scrollWidth <= clientWidth` is `true`.

Screenshots: `04-pull-requests-desktop.png`, `04-pull-requests-phone.png`.

## 5. Waits on a merge

- Row 14 is inside `[data-section="waits-on-merge"]`. Its text contains `Waits on a merge on GitHub. PL-3 moves to Done when GitHub reports the merge.`, and its `Merge on GitHub` href is `https://github.com/gannonh/punchlist-sandbox/pull/14`. Row 7 has no notice.
- `closed` with `merged: true` delivered for 14 while the page was open. Row 14 was gone after 52 ms, and `SELECT status FROM issue WHERE id = 'PL-3'` is `done`.

Screenshot: `05-waits-on-merge-row.png`.

## 6. Live: appears and moves

With the page open:

- `opened` for 11 on `feature/pl-3-c` showed `[data-section="in-review"] [data-pr-row="11"]` after 462 ms.
- `converted_to_draft` moved it into `[data-section="draft"]` after 410 ms.
- `closed` without a merge removed it after 461 ms.

```
 seq | issue_id |     kind
-----+----------+--------------
  12 | PL-1     | pull_request
  13 | PL-2     | pull_request
  14 | PL-3     | pull_request
  15 | PL-2     | pull_request
  19 | PL-3     | pull_request
  20 | PL-3     | pull_request
  21 | PL-3     | pull_request
```

Screenshot: `06-pull-request-11-in-review.png`.

## 7. Live: checks, threads

A `check_run` `completed` `success` on PR 7's head and an `unresolved` `pull_request_review_thread` for 7. Row 7 read `checks 1/1` and `1 open thread` after 413 ms.

```
 checks_passed | checks_total | open_threads
---------------+--------------+--------------
             1 |            1 |            1
```

## 8. Unlinked

- The `names-no-issue` rows are `[9]`.
- `$C pl "$RUN" pr unlinked` prints `#9  open  stray  Bump deps`.
- Enter on focused row 9 called the stubbed `window.open` once with `["https://github.com/gannonh/punchlist-sandbox/pull/9","_blank","noopener"]`, and the path stayed `/pull-requests`.
- `opened` delivered for the unlinked PR 12 (`stray-two`). It was absent 2.5 s later. It was still absent 2 s after `pl issue comment PL-1 --body ping`, and it was present after a reload.

The check passes through the reload. The comment did not bring 12 in, because a `comment` event does not refetch the pull requests. The issue's Perf budget asks for exactly that: only `pull_request`, `transition` and `workflow` events refetch, so a burst of comment or run events costs nothing. A `pull_request`, `transition` or `workflow` event for any issue does bring it in. Check 5's merge, for example, is a `pull_request` event.

## 9. Links and plain text

- Row 7's `Open on GitHub` has href `https://github.com/gannonh/punchlist-sandbox/pull/7`, target `_blank` and rel `noopener noreferrer`.
- Enter on focused row 7 opened `/issues/PL-1`.
- PR 13 was titled `<img src=x onerror="window.__x=1">`, with its url set to `javascript:alert(1)` by `$C sql`. After a reload its row text holds the title literally. The row has 0 `img` elements and 0 links, and `typeof window.__x` is `undefined`.

Screenshot: `09-plain-text-title.png`.

## 10. Render budget and request set

Runs `pl68-perf` (branch) and `pl68-main` (main), each seeded the same way:

- 500 issues through `POST /api/issues`.
- 50 open pull requests linked to PL-1..PL-50, every fifth a draft.
- 5000 closed or merged pull requests, with `generate_series`.

Results:

- On `/`, the `/api/pull-requests` request started after `/api/issues` ended on every load. On the first load `/api/issues` ended at 77.6 ms and `/api/pull-requests` started at 97.9 ms.
- The list's first `pl:render` with `rows: 500` had a median over 15 loads of 19.8 ms on the branch and 20.2 ms on main, a 2 % difference. In an earlier pair of runs it was 21.9 ms against 21.1 ms, a 3.8 % difference.
- The page's first `pl:render` with `layout: 'pull-requests'` and `rows: 50` had a median over 5 loads of 7.9 ms. The budget is 100 ms.
- 100 timed `GET /api/pull-requests` calls (curl `time_total`): p50 4.0 ms, p95 4.9 ms, max 6.5 ms. The budget is a p95 under 50 ms.
- Plan: without an index, Postgres scanned `pull_request` sequentially (`Rows Removed by Filter: 5000`, execution 1.24 ms). As the issue says to do in that case, the PR adds `pull_request_open ON pull_request (repository_id, number DESC) WHERE state = 'open'`. With it, the plan is `Index Scan using pull_request_open`, execution 0.20 ms. The p95 above was measured with the index.

Screenshot: `10-list-header-with-count.png`.

## Video

`pl-68-walkthrough.webm` (39 s), recorded on `pl68-branch`:

1. Sign in, then `g` `p`.
2. `ready_for_review` moves #8 from Draft to In review.
3. #15 for PL-3 in Merging shows the waiting notice and `Merge on GitHub`, and its merge removes the row.
4. Enter on #7 opens PL-1, `Esc` goes to the list, and the header item goes back.
5. The phone layout.
