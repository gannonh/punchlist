PL-176 final independent live checks — PR #148

Head: d00dd209496bb88a62044e45d9d7de5540a9682d, after merging main to fix Punchlist's `mergeable` gate. Main baseline: 63b48533e0cb2303d88d9e52d24216816de758a3. All checks below were rerun after the final push. The main update contains only runner cleanup changes; the reviewed UI diff remains the class rename and width regression test.

Command: `PUNCHLIST_DB_PORT=5549 COMPOSE_PROJECT_NAME=pl176-review-current node verify-artifacts/pl176-final.cjs`. Each numbered check starts and stops its own fresh verify-punchlist instance. The main baseline also uses a fresh main instance and main web build. Real HTTP APIs and Chromium drive all interactions, with no model calls or direct database writes.

1. PASS — Same `Waits.` flow at 1280 px: main body starts 62 px right of the header; branch offset is 0 px. Branch header/fields/body x=819, width=443.
2. PASS — Fields grid and header both measure 443 px at 1280 px. Two grid columns remain 110 px and 321 px, separated by 12 px.
3. PASS — 2000-character body measures 443 px, matching the header; sheet scrollWidth/clientWidth = 479/479.
4. PASS — Real runner registration/claim/waiting/finish API flow produces the waiting notice. Header, notice, fields and body all measure x=819, width=443. Database reads confirm the waiting reason and finished run.
5. PASS — At 390 px, short body and waiting issue align at x=19 with width=353 for header, fields, body and notice. Main's short body offset remains +62 px.
6. PASS — With the waiting sheet open at 390 px, document.documentElement.scrollWidth is 390.
7. PASS — Empty-body sheet has fields width=353, matching its header, and no body/rule container.
8. PASS — Editing title through the sheet returns PATCH 200; PATCH /api/issues/PL-1 updates the body to `Updated body.`, visible in the still-open sheet at width=443. GET and SQL reads confirm the saved title and body length.
9. PASS — Main and branch builds render the same rows with a fixed clock at 1280 px. Issue page, inbox and Pull requests page each have raw RGBA diff 0 and pixelmatch diff 0 (threshold 0.1, zero permitted differing pixels), without masks.
10. PASS — Finalized 35.520-second WebM opens the short-body, long-body and waiting issues at 1280 and 390 px. Attached separately as proof of kind `video`.

Raw measurements, screenshots, SQL reads, harness and recording: [final evidence](https://github.com/gannonh/punchlist/tree/assets/PL-176/PL-176/codex-final). This report supersedes the pre-main-update results on 09b4822. All ten final checks completed without errors or retries. `git diff --check` passes. The regression e2e test also passed locally before the main update; GitHub CI is tracked separately on the final head.

Review: complete UI diff inspected; no additional code fixes needed. No unresolved review threads were present, so no replies were required. No bot review was requested.
