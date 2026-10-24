# Projects screen: milestones, PASS when and progress

A signed-in person opens `/w/<prefix>/projects` and sees each project of the workspace, oldest first, with its milestones, oldest first. Each milestone shows its name, its `PASS when` text with its line breaks (or `No PASS when yet`), `n of m done` and a bar split by status. An issue in `done` is done; an issue in `canceled` is left out of the total. Opening a milestone lists its issues that are not canceled, each linking to its issue page, and `Show in list` opens the list filtered to the milestone. The screen is read-only: projects and milestones are made with `pl` ([issue-placement.md](issue-placement.md)), and the counts follow the event stream.

## Sub-features

- `projects-screen`: `/w/<prefix>/projects` (and `/projects`, which opens the workspace last used). One `section[data-project=<id>]` per project with an `h2` of its name, and one `article[data-milestone=<id>]` per milestone. `No projects yet.` (`[data-empty]`) with none; `No milestones yet.` in a project with none.
- `projects-pass-when`: `[data-pass-when]` holds `PASS when` and the text, `white-space: pre-line`, as React text (no HTML), or `No PASS when yet`.
- `projects-progress`: `[data-done-count]` reads `n of m done`; `[data-canceled]` reads `k canceled, not counted` when k is above 0; `[data-bar]` holds one `span[role=img][data-status][data-count]` per status with counted issues, in the workflow's order, named `<Status name>: <count>`. Computed from the cached list rows by `milestoneProgress` (`web/src/model/projects.ts`), so a workflow without the keys `done` and `canceled` shows no progress.
- `projects-open`: the milestone's name is a `button[aria-expanded]`; open, `[data-issues]` lists `a[data-issue-link=<id>]` to `/issues/<id>` with the status name, canceled issues left out.
- `projects-show-in-list`: `a[data-show-in-list]` to `/w/<prefix>?milestone=<id>`, where the list's milestone select reads `<project> · <milestone>`.
- `projects-keys`: the header's `Projects` link after `Inbox`; `g` then `m` within a second opens the screen from the list, the board, the Pull requests page, the inbox and the screen (not in a field, nor with a dialog or menu open); `Esc` goes to the list.
- `projects-perf`: each commit of the screen with its data records `pl:render` with `detail: {layout: 'projects', milestones}`.

## How to get to it (user POV)

- The app signed in (`$C sign-in-url "$RUN"`): the header's `Projects` link, or `g` then `m`.
- Data: `$C pl "$RUN" project create`, `milestone create`, `issue edit --project --milestone`.

## Driving it with control-punchlist

Preconditions:

- `pnpm build`, a run started with `RUN=$($C up)` (the built-in workflow), `source /tmp/punchlist-verify/$RUN/run.env`, and a headless Chromium on sartre (Playwright from `node_modules/.pnpm/node_modules/playwright`) signed in through `$C sign-in-url "$RUN"`.

- **Setup.** `$C pl "$RUN" project create Punchlist`; `$C pl "$RUN" milestone create "Gate 1" --project Punchlist --pass-when $'Line one\nLine two'`; `$C pl "$RUN" milestone create "Gate 2" --project Punchlist`; `$C pl "$RUN" issue create --title <title>` four times (PL-1 to PL-4); `$C pl "$RUN" issue edit PL-<n> --project Punchlist --milestone "Gate 1"` for PL-1, PL-2 and PL-3; then `$C sql "$RUN" "UPDATE issue SET status = 'done' WHERE id = 'PL-1'"` and `... 'canceled' WHERE id = 'PL-3'` (no workflow path reaches them without GitHub).
- **Order.** `$C sql "$RUN" "SELECT m.name FROM milestone m ORDER BY created_at"` against `[...document.querySelectorAll('[data-milestone] button[aria-expanded]')].map(b => b.textContent)`.
- **Progress.** `$C sql "$RUN" "SELECT status, count(*) FROM issue WHERE milestone_id = (SELECT id FROM milestone WHERE name = 'Gate 1') GROUP BY status"` (`done 1`, `canceled 1`, `backlog 1`) against `[data-done-count]` (`1 of 2 done`) and the bar's `aria-label`s (`Backlog: 1`, `Done: 1`).
- **Live counts.** With the screen open, `$C pl "$RUN" issue edit PL-4 --project Punchlist --milestone "Gate 1"`; `[data-done-count]` reads `1 of 3 done` within 2 s, and `$C sql "$RUN" "SELECT count(*) FROM event WHERE kind = 'edited'"` is one higher.
- **Text from outside.** `$C pl "$RUN" milestone create XSS --project Punchlist --pass-when '<img src=x onerror=alert(1)>'`, reload: the text shows as characters and `document.querySelectorAll('img').length` is `0`.
- **Phone.** At 390 px, `document.documentElement.scrollWidth <= document.documentElement.clientWidth`, and each `[data-pass-when] span:last-child` has `scrollHeight <= clientHeight`.
- **Theme.** `grep -nE '#[0-9a-fA-F]{3,8}\b|rgba?\(' web/src/screens/projects.tsx` prints nothing.
- **E2E.** `web/e2e/projects.spec.ts`; unit `web/src/model/projects.test.ts`.

## Gotchas

- The setup's `UPDATE issue` rows are the exception to the read-only SQL rule, since only GitHub reaches `done` and `canceled` here; the issue's live checks name them. An `UPDATE` writes no event, so an open screen shows it only after a reload.
- A milestone made with `pl` after the screen loaded shows on reload or on the next visit: the screen reads `GET /api/projects` when it opens, and the stream carries issues, not projects.
- The list hides Done and Canceled by default, so `Show in list` on a milestone shows only its open issues.
