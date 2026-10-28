PL-176 independent review — PR #148

Head: 09b48220d9719870057a6a57524eecd455f27da2. Main baseline: 63b48533e0cb2303d88d9e52d24216816de758a3.

I reviewed the complete branch diff and reran the live checks myself using the repository's verify-punchlist skill. Checks use fresh isolated databases and real HTTP API/browser paths, without model calls. Raw measurements, database reads, screenshots and the harness are preserved under assets/PL-176/PL-176/codex-review. No code changes were needed. There were no unresolved review threads to answer.

1. PASS — Same short-body flow on main and branch at 1280 px. Main: body starts 62 px right of the header. Branch: header, fields and body start at x=819 and are 443 px wide.
2. PASS — Fields grid is 443 px, exactly the header width at 1280 px. Grid keeps two columns (110 px and 321 px, with a 12 px gap).
3. PASS — 2000-character body ("Long body " repeated 200 times) is 443 px wide, matching the header. Sheet scrollWidth/clientWidth = 479/479.
4. PASS — Waiting notice, fields and body all start at x=819, with width 443 px. Waiting status was produced through runner registration, claim, waiting and finish API calls; saved SQL reads confirm the waiting reason.
5. PASS — At 390 px, short body and waiting issue have header, fields, body and notice aligned at x=19 and width 353 px. Main's short body remains displaced 62 px from its header.
6. PASS — At 390 px, document.documentElement.scrollWidth = 390 with the waiting sheet open.
7. PASS — Empty-body sheet has a full-width 353 px fields grid and no body/rule container.
8. PASS — Editing the title through the sheet returns PATCH 200. Updating the body through PATCH /api/issues/PL-1 renders "Updated body." in the still-open sheet at width 443 px. GET and database reads confirm the saved title and body length.
9. PASS — Main and branch builds render the same database with a fixed clock. Issue page, inbox and Pull requests page each have pixelmatch diff 0 (threshold 0.1, max differing pixels 0; no masks). Raw RGBA differences are recorded separately in results.json. An earlier byte-for-byte comparison found 22 pixels differing by one channel value at rounded panel corners. The initial attempt was interrupted when my simultaneous e2e build temporarily removed dist/index.html; I reran the entire check after the build completed. Commands: `PUNCHLIST_DB_PORT=5549 COMPOSE_PROJECT_NAME=pl176-review-current node verify-artifacts/pl176-audit.cjs`, then `MIN_CHECK=9` with the same command for checks 9–10.
10. PASS — A finalized 35.040-second WebM opens the short-body, long-body and waiting issues at both 1280 and 390 px. Attached to PL-176 as video proof 5b9c84bf-d6f4-4181-bc89-ee8c39bddabf (2,393,612 bytes). The final check 9 screenshots also have raw RGBA diff 0 on all three screens.

Additional validation: `PUNCHLIST_DB_PORT=5549 COMPOSE_PROJECT_NAME=pl176-review-current pnpm e2e --grep 'fields and body share'` passed (1 test, covering both viewport widths). `git diff --check` passed. GitHub check, compose and e2e checks pass on this head; CodeRabbit is skipped, and no bot review was requested.
