Independent agent review of PL-167 / PR #147

Head tested: c98b73ab09cd80ce884ecf244c8667533a0d34e0. No implementation changes were needed. CI passes; Punchlist listed zero unresolved review threads, so no thread replies were necessary.

I ran these checks myself with fresh verify-punchlist databases and Playwright Chromium. Mobile contexts used 390×844, hasTouch:true, isMobile:true. The baseline served origin/main's web build (6ca3793) from a separate worktree; the branch served the PR head. Both used the verification server on this branch. Desktop contexts used 1280×800 and confirmed a fine pointer. Screenshots and raw observations are on assets/PL-167 under PL-167/independent/.

| Check | Result | Observation |
| --- | --- | --- |
| 1. Main versus branch | PASS | Main had zero footer buttons; two j-text taps left PL-1 focused. Branch started on PL-1, two next taps focused PL-3, and Open navigated to /issues/PL-3. |
| 2. Status by tap | PASS | Tapped PL-2, Status, Todo. GET /api/issues/PL-2 returned todo. GET /api/issues/PL-2/events contained exactly one transition, backlog → todo, by Verify Person (person). A SQL read confirmed that row and actor. |
| 3. No focus | PASS | With an empty filtered list, status, assignee, labels, project and milestone each had disabled and title="Focus an issue first". |
| 4. Range selection | PASS | Select, Select range, Next twice, Select showed 3 selected. Labels → touch-fixture sent exactly three PATCH requests; the toast named 3 issues. Subsequent GETs confirmed the label removed from all three issues. |
| 5. Inbox | PASS | With two Todo items, Next twice focused the second item (PL-2). Open navigated to /issues/PL-2 and GET /api/inbox returned unread:false for it. |
| 6. Pull requests / issue | PASS | Pull requests → Esc returned to /w/PL. On the issue page, Gates positioned #gates at y=61.89; Activity positioned #timeline at y=486.25, both inside the 844 px viewport. |
| 7. Keyboard only for typing | PASS | Individually tapped all 16 list and four issue-page strip buttons, reloading between actions. Only Filter focused INPUT and Comment focused TEXTAREA; other actions left BODY or a menu DIV active. |
| 8. Strip clearance | PASS | With 60 issues, scrolling to the end put the last row bottom at 763.73 and strip top at 786.73. All 16 buttons were 44 px high. |
| 9. Desktop | PASS | pixelmatch with threshold:0 returned 0 differing pixels for both complete list and issue-page screenshots. A fixed browser clock controlled elapsed-time labels. j, k, s, x, g, t, c and Escape assertions passed on main and the branch. |
| 10. Phone handoff / recording | PARTIAL — not counted as fully passed | Started the branch instance on sartre's Tailscale address with PUNCHLIST_VERIFY_HOST=$(tailscale ip -4). Recorded 34.2 seconds of navigation, status → Todo and typing-focus checks at 390 px; attached the new video as video proof. The instance URL was http://100.96.231.20:40353 during this attempt. A lasting URL and token for Gannon are still unavailable: runner cleanup removes attempt-scoped servers and credentials. The verification skill requires token secrecy and cleanup. The PR's existing phone setup command remains available; a person must start an instance outside this runner attempt and share access privately for the actual phone check. |

Commands used:

```sh
pnpm build
PUNCHLIST_DB_PORT=5436 PUNCHLIST_VERIFY_HOST=$(tailscale ip -4) .claude/skills/verify-punchlist/bin/control-punchlist up pl167-independent
# Separate fresh main/frontend baseline and desktop instances were also started with control-punchlist up.
node verify-artifacts/pl167-independent/check.cjs
CHECKS=9 BRANCH_RUN=pl167-desktop-branch MAIN_RUN=pl167-desktop-main node verify-artifacts/pl167-independent/check.cjs
pnpm --filter @punchlist/web test src/model/shortcuts.test.ts
# With E2E_BASE_URL and E2E_CONFIG pointing to the fresh instance:
pnpm --filter @punchlist/web e2e shortcut-strip.spec.ts
gh pr checks 147 --repo gannonh/punchlist
```

Unit tests: 10 passed. Shortcut-strip E2E: 2 passed. CI: check, compose and e2e passed; CodeRabbit skipped. No bot review was requested.

Harness corrections during verification: the first draft incorrectly asserted that section shortcuts change the URL hash (they scroll to DOM ids), requested a nonexistent /timeline endpoint instead of /events, and checked a row action before data loaded. The first desktop image comparison differed only in live elapsed-time labels; the fresh rerun with a fixed clock returned zero pixel differences. These were verification issues; no product failure was found.

Skill used: [.claude/skills/verify-punchlist/SKILL.md](https://github.com/gannonh/punchlist/blob/c98b73ab09cd80ce884ecf244c8667533a0d34e0/.claude/skills/verify-punchlist/SKILL.md).
