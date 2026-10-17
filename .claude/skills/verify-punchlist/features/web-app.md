# Web app: the issue list and board

A signed-in person opens the server's URL and sees the workspace's issues grouped by status (List, key `1`) or in one column per status (Board, key `2`). Each row shows the id, title, labels, assignee (a person, an agent with its kind, or Unassigned), run state, pull request state with checks, and the gate summary of the next gated transition. Statuses no agent may act in carry a "people only" badge. The list stays current through the workspace's event stream: a move made with `pl` shows without a reload.

## Sub-features

- `web-sign-in`: `/sign-in` takes a person's token and sets the `punchlist_session` cookie; any other token answers `That is not a person’s token.`
- `web-list`: one section per non-empty status (`[data-group=<key>]`), in workflow order, rows `[data-issue=<id>]`. Each row holds `[data-assignee]`, `[data-run]`, `[data-pr]` and `[data-gates]`.
- `web-board`: one column per workflow status (`[data-column-section=<key>]`), empty ones showing `No issues`. Below 768 px one column shows at a time, picked from a status strip (`[role=tab][data-column]`).
- `web-filter`: the `Filter issues` box matches id, title, labels, assignee and status; the `Assignee filter` select takes Everyone, People, Agents, one agent kind, or Unassigned. Nothing matching shows `[data-empty]` with `Nothing here` and `Clear filters`. The count reads `<shown> of <total>` (`[data-count]`).
- `web-keys`: `j`/`k` (or arrows) move the selection (`aria-selected="true"` on one `[data-issue]`), `Enter` opens the issue's side sheet, `s` opens the status menu, `/` focuses the filter, `1`/`2` switch layout (`?layout=board` in the URL). On the board `h`/`l` request a move of the selected issue one column left or right through `POST /api/issues/{id}/transitions`. Keys do nothing while a field, the sheet, a menu or the New issue dialog has focus.
- `web-create`: `New issue` opens a dialog (Title, Body); `Create issue` adds the row to Backlog when the server answers.
- `web-edit`: the side sheet edits Title, Labels and Parent (Enter or blur saves; an empty Parent clears it, and the parent's title shows under it), and Assignee, Project (`No project` or one of `GET /api/projects`) and Milestone (`No milestone` or one of the issue's project's, disabled without a project, its `PASS when` text under it). Another project clears the milestone. Edits show at once; a refused edit puts the old value back and toasts `<id> was not changed. Refused (<code>): <message>`. The selects are `[aria-label=Project]` and `[aria-label=Milestone]`; their options have role `option`. Projects and milestones come from `pl` ([issue-placement.md](issue-placement.md)).
- `web-transition`: a status change is not optimistic. The row shows `[data-transition=pending]` `Moving to <Status>…` until the server answers, then either moves or shows `[data-transition=refused]` `<Status>: Refused (<code>): <message>` (for `gate_failed`: `Refused (gate_failed): gate <gate> failed: <reason>`), and stays where it was.
- `data-last-seq` on the screen's root element is the last event sequence the app's cache applied, from the list or the stream. Compare it to `SELECT max(seq) FROM event`.
- `web-perf`: each commit of the list or board records a `pl:render` measure (`detail: {layout, rows}`), from the start of the view's render to its DOM update. Layout and paint are not included.

## How to get to it (user POV)

- The run's server URL, `SERVER_URL` in `run.env`, serving `web/dist`.
- `pnpm dev` serves the same app from Vite on `127.0.0.1:5173`, proxying `/api` to the server.

## Driving it with control-punchlist

Preconditions:

- `pnpm build` has written `web/dist` (the run links it, so a later rebuild is served on reload).
- A run started with `RUN=$($C up)`; for the T3 Code integrated browser, which runs on the Mac mini and cannot reach sartre's 127.0.0.1, start it bound to sartre's Tailscale address: `RUN=$(PUNCHLIST_VERIFY_HOST=$(tailscale ip -4) $C up)`.

- **Sign in.** `URL=$($C sign-in-url "$RUN")` prints `$SERVER_URL/sign-in-<random>.html`. Opening it posts the run's person token to `/api/session` and lands on `/` signed in. The token is never in the URL or the transcript; the page is removed after 2 minutes, so get a new URL per browser.
- **Headless.** Playwright from this repository (`node_modules/.pnpm/node_modules/playwright`) with the Chromium in `~/.cache/ms-playwright`: `page.goto(URL)`, then `page.waitForURL(u => u.pathname === '/')`. Read state from the DOM selectors above and from `$C sql`.
- **Integrated browser.** Load the preview tools with ToolSearch `select:mcp__t3-code__preview_open,mcp__t3-code__preview_click,mcp__t3-code__preview_evaluate,mcp__t3-code__preview_snapshot,mcp__t3-code__preview_type,mcp__t3-code__preview_press,mcp__t3-code__preview_resize,mcp__t3-code__preview_recording_start,mcp__t3-code__preview_recording_stop`, then `preview_open` the sign-in URL. Assert with `preview_evaluate`, for example `[...document.querySelectorAll('[data-group]')].map(e => e.dataset.group)`.
- **An issue in In Progress, without a model.** As the person: create it, move it to `todo` and `start`, then `$C curl "$RUN" -X POST -H 'content-type: application/json' -d '{"name":"r","agents":["claude-code"]}' "$SERVER_URL/api/runners"` and claim with the answer's `token`: `curl -s -X POST -H "Authorization: Bearer <runner token>" -H 'content-type: application/json' -d '{}' "$SERVER_URL/api/runs/claim"`. Keep the runner token out of evidence. `web/e2e/issues.spec.ts` does the same in code.
- **Pending then refused.** Select that issue, press `s`, pick Agent Review. Pass when `[data-transition=refused]` reads `Agent Review: Refused (role_not_allowed): only an agent makes in_progress → agent_review, not a person` and `$C sql "$RUN" "SELECT count(*) FROM transition WHERE to_status = 'agent_review'"` is `0`. The server answers in milliseconds; to see the pending state, hold the request (`page.route`, as the e2e test does).
- **Live stream.** With the app open, `$C pl "$RUN" issue move PL-1 todo`. Pass when `[data-group=todo] [data-issue="PL-1"]` exists within 2 s.
- **Phone.** Resize to 390×844 on both layouts. Pass when `document.documentElement.scrollWidth <= document.documentElement.clientWidth`.
- **Render budget.** Seed issues through `POST /api/issues` (500 sequential requests take about 2 s), reload, then read `performance.getEntriesByName('pl:render').map(m => [m.duration, m.detail])`. The first entry with `rows: 500` is the list's first render after load.
- **E2E.** `pnpm e2e` builds the app, starts its own run, runs `web/e2e/` and stops the run.

## Gotchas

- The workflow has no transition to the column on the left of most statuses (no `todo → backlog`), so `h` usually shows a `no_transition` refusal; that is the transition endpoint answering, not a bug.
- The server looks for `index.html` once, at start. A run started before `pnpm build` ever ran serves only the API (`up` warns); build, then `down` and `up` again.
- The session cookie is `SameSite=Strict` and `Secure` only behind https, so plain http on the Tailscale address works. A browser on another origin (such as Vite on a different port with the API elsewhere) does not send it; use the run's URL or `pnpm dev`'s proxy.
- Run and pull request changes write no event yet (KAT-3777), so the open app shows them on reload or the issue's next event.
