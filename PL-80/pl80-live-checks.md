# PL-80 live checks

Branch commit 9c7ba08, main d78d338. No LLM call: a runner registered through the API, runs claimed and finished by hand.

## Check 1 on main
```
== 1. Same flow on main
  PL-1: claimed run 2f3ffd8e-d6b5-4fa2-957e-e7023d76df01, finished {"outcome":"succeeded","reason":null}
  PL-1: claimed run 62d7a6c1-de46-4d1d-9868-d6d409c5b0ec, finished {"outcome":"succeeded","reason":null}
  PL-1: claimed run ccee7699-e7cb-4755-83c5-577ce6ff2066, finished {"outcome":"succeeded","reason":null}
  GET /api/inbox: []
PASS 1 (main) PL-1 run items: 0
```
## Checks 1-7 on the branch
```
== 1. Same flow on branch
  PL-1: claimed run e2af4a04-87cf-4f54-97b0-f26ae6690a5f, finished {"outcome":"succeeded","reason":null}
  PL-1: claimed run 2df93558-42cd-4120-bfb5-f602ea899975, finished {"outcome":"succeeded","reason":null}
  PL-1: claimed run e289cc65-4368-47e2-b40b-e61752f047e7, finished {"outcome":"succeeded","reason":null}
  GET /api/inbox: [{"kind":"run","id":"PL-1"}]
PASS 1 (branch) PL-1 run items: 1
== 2. Limit with success
[
  {
    "kind": "run",
    "issue": {
      "id": "PL-1",
      "title": "One",
      "status": "in_progress",
      "status_name": "In Progress"
    },
    "agent": "claude-code",
    "runs": 3,
    "last_outcome": "succeeded",
    "reason": null,
    "limit_reached": true,
    "since": "2026-10-24T21:57:30.314036Z",
    "since_seq": 14,
    "unread": true
  }
]
PASS 2 fields: [true,"succeeded",3,"claude-code"]
PASS 2 pl issue show: waiting on a person: 3 runs in In Progress ended without moving it
PASS 2 runs of PL-1: 3
== 3. Limit with failure
  PL-2: claimed run ca0ddbc7-8a1d-4c43-bd4a-641eb2ac3b6d, finished {"outcome":"succeeded","reason":null}
  PL-2: claimed run b42d2809-0fdb-4486-aac8-56708486fbcc, finished {"outcome":"succeeded","reason":null}
  PL-2: claimed run 36194fb8-14d7-4e1d-8d77-9ba43ea1ddc5, finished {"outcome":"failed","reason":"agent crashed"}
[
  {
    "kind": "run",
    "issue": {
      "id": "PL-2",
      "title": "Two",
      "status": "in_progress",
      "status_name": "In Progress"
    },
    "agent": "claude-code",
    "runs": 3,
    "last_outcome": "failed",
    "reason": "agent crashed",
    "limit_reached": true,
    "since": "2026-10-24T21:57:30.679701Z",
    "since_seq": 23,
    "unread": true
  }
]
PASS 3 fields: ["failed","agent crashed",true]
PASS 3 items for PL-2: 1
== 4. Not yet
  PL-3: claimed run 247d746b-c83c-443e-b390-2e8905208fd1, finished {"outcome":"failed","reason":"agent crashed"}
PASS 4 queued dispatch: 1
PASS 4 PL-3 items while queued: 0
PASS 4 PL-3 row run.state: running
PASS 4 PL-3 items while running: 0
  PL-3: claimed run 9574ee8d-c5bf-470b-a5ec-bba2166d150a, finished {"outcome":"succeeded","reason":null}
== 5. Audience
  second person's inbox: []
PASS 5 second person PL-1/PL-2 run items: 0
PASS 5 first person run items: ["PL-1","PL-2","PL-3"]
PASS 5 owner of PL-1's agent: f2c55467-5a26-4040-91b9-b0e7c04f555e
== 6. Leaving
PASS 6 PL-1 status: todo
PASS 6 PL-1 run items: 0
PASS 6 PL-1 waiting items: 1
  unread 3 -> 3 (PL-1's run item was unread: true; its waiting item is unread: true)
== 7. Read
PASS 7 read answer: 204
PASS 7 unread fell by 1: 2
PASS 7 inbox_read rows: run:23
```
## Checks 8-10 (headless Chromium, one recording)
```
PASS 8 header count rose by 1 without a reload: 1
     2 -> 3 in 540 ms
PASS 9 PL-3 lines: ["3 runs in In Progress ended without moving it"]
PASS 9 PL-3 agent: ["Claude Code"]
PASS 9 PL-3 link: "/issues/PL-3"
PASS 9 PL-2 lines: ["3 runs in In Progress ended without moving it","Run failed: agent crashed"]
PASS 9 run items in order: ["PL-5","PL-4","PL-3","PL-2"]
PASS 10 PL-5 reason as text: ["3 runs in In Progress ended without moving it","Run failed: <img src=x onerror=alert(1)>"]
PASS 10 img elements in the item: 0
PASS 9 no overflow at 390 px: true
PASS 9 Enter opens the issue: "/issues/PL-3"
PASS 9 opening PL-3 read it: 3
```
Check 9 uses PL-3 for the success sentence: check 6 moved PL-1 to Todo first, as the issue orders them.

## Perf budget (debug build)
1000 issues seeded through the API, 100 in Todo, 50 with three ended runs.
```
-- main, then branch, before ANALYZE
issues 1005, todo 100, runs 153
GET /api/inbox: 0.010806 0.008752 0.008786 0.010491 0.007951 median 0.008786 s
GET /api/issues: 0.114573 0.110286 0.131398 0.118007 0.123459 median 0.118007 s
inbox items: [100,0]
issues 1005, todo 101, runs 165
GET /api/inbox: 0.010858 0.009723 0.010551 0.012869 0.011340 median 0.010858 s
GET /api/issues: 0.056306 0.055066 0.056470 0.054213 0.055519 median 0.055519 s
inbox items: [155,54]
-- after ANALYZE, alternating
== pl80m (after ANALYZE)
GET /api/inbox: 0.006901 0.005333 0.005100 0.005086 0.005542 median 0.005333 s
GET /api/issues: 0.054625 0.053190 0.062861 0.057970 0.062163 median 0.057970 s
== pl80b (after ANALYZE)
GET /api/inbox: 0.020759 0.021801 0.019591 0.017819 0.018219 median 0.019591 s
GET /api/issues: 0.074968 0.064082 0.058220 0.058875 0.066316 median 0.064082 s
== pl80m (after ANALYZE)
GET /api/inbox: 0.006400 0.007246 0.006777 0.006286 0.007288 median 0.006777 s
GET /api/issues: 0.053271 0.049525 0.050208 0.051930 0.067423 median 0.051930 s
== pl80b (after ANALYZE)
GET /api/inbox: 0.014761 0.016188 0.011688 0.012808 0.011575 median 0.012808 s
GET /api/issues: 0.057671 0.051339 0.051729 0.054960 0.051619 median 0.051729 s
== pl80m
GET /api/issues: 0.047051 0.044950 0.045683 0.047385 0.044976 median 0.045683 s
== pl80b
GET /api/issues: 0.046783 0.049035 0.050663 0.049550 0.050026 median 0.049550 s
== pl80m
GET /api/issues: 0.045963 0.045330 0.050579 0.046573 0.048100 median 0.046573 s
== pl80b
GET /api/issues: 0.047116 0.046534 0.046962 0.046064 0.047993 median 0.046962 s
== pl80m
GET /api/issues: 0.045575 0.046756 0.048855 0.046254 0.047166 median 0.046756 s
== pl80b
GET /api/issues: 0.062760 0.070641 0.052931 0.050107 0.051050 median 0.052931 s
-- both servers on main's database, median of 11 each round
round 1  main 0.046138  branch 0.046482
round 2  main 0.045390  branch 0.045392
round 3  main 0.044977  branch 0.045960
round 4  main 0.046305  branch 0.045770
round 5  main 0.044779  branch 0.044891
```
## Scripts
### api-checks.sh
```
#!/usr/bin/env bash
# PL-80 live checks 1-7 through the API. Usage: api-checks.sh <control-punchlist> <RUN> [main]
# With `main`, runs check 1's flow only (main has no run items).
set -uo pipefail
C=$1 RUN=$2 MODE=${3:-branch}
source /tmp/punchlist-verify/$RUN/run.env
P() { "$C" curl "$RUN" "$@"; }                      # as the person
J='content-type: application/json'
RT=$(P -X POST -H "$J" -d '{"name":"r","agents":["claude-code"]}' "$SERVER_URL/api/runners" | jq -r .token)
R() { curl -s -H "Authorization: Bearer $RT" -H "$J" "$@"; }  # as the runner; token kept out of output
sql() { psql "$DATABASE_URL" -XtAc "$1"; }
claim() { R -d '{"wait_seconds":0}' "$SERVER_URL/api/runs/claim"; }
finish() { R -d "$2" "$SERVER_URL/api/runs/$1/finish" >/dev/null; }
OK='{"attempt":1,"outcome":"succeeded","duration_ms":1}'
fail_with() { jq -cn --arg r "$1" '{attempt:1,outcome:"failed",reason:$r,duration_ms:1}'; }
# Claims the next run, checks it is for $1, finishes it with $2.
run_once() {
  local c id; c=$(claim); id=$(jq -r .run_id <<<"$c")
  [ "$(jq -r .issue.id <<<"$c")" = "$1" ] || { echo "claim got $(jq -c .issue.id <<<"$c"), wanted $1"; exit 1; }
  finish "$id" "$2"; echo "  $1: claimed run $id, finished $(jq -c '{outcome,reason}' <<<"$2")"
}
inbox() { P "$SERVER_URL/api/inbox"; }
item() { inbox | jq -c --arg id "$1" --arg k "${2:-run}" '[.items[] | select(.issue.id == $id and .kind == $k)]'; }
pass() { if [ "$2" = "$3" ]; then echo "PASS $1: $2"; else echo "FAIL $1: got $2, want $3"; fi; }
mv() { "$C" pl "$RUN" issue move "$1" "$2" >/dev/null; }

for t in One Two Three Four Five; do "$C" pl "$RUN" issue create --title "$t" >/dev/null; done
mv PL-1 todo; mv PL-1 start
echo "== 1. Same flow on $MODE"
for i in 1 2 3; do run_once PL-1 "$OK"; done
echo "  GET /api/inbox: $(inbox | jq -c '[.items[] | {kind, id: .issue.id}]')"
if [ "$MODE" = main ]; then
  pass "1 (main) PL-1 run items" "$(inbox | jq '[.items[] | select(.issue.id == "PL-1")] | length')" 0
  exit 0
fi
pass "1 (branch) PL-1 run items" "$(item PL-1 | jq length)" 1

echo "== 2. Limit with success"
item PL-1 | jq .
pass "2 fields" "$(item PL-1 | jq -c '.[0] | [.limit_reached, .last_outcome, .runs, .agent]')" '[true,"succeeded",3,"claude-code"]'
pass "2 pl issue show" "$("$C" pl "$RUN" issue show PL-1 | grep -o 'waiting on a person: .*')" "waiting on a person: 3 runs in In Progress ended without moving it"
pass "2 runs of PL-1" "$(sql "SELECT count(*) FROM run WHERE issue_id = 'PL-1'")" 3

echo "== 3. Limit with failure"
mv PL-2 todo; mv PL-2 start
for i in 1 2; do run_once PL-2 "$OK"; done
run_once PL-2 "$(fail_with 'agent crashed')"
item PL-2 | jq .
pass "3 fields" "$(item PL-2 | jq -c '.[0] | [.last_outcome, .reason, .limit_reached]')" '["failed","agent crashed",true]'
pass "3 items for PL-2" "$(inbox | jq '[.items[] | select(.issue.id == "PL-2")] | length')" 1

echo "== 4. Not yet"
mv PL-3 todo; mv PL-3 start
run_once PL-3 "$(fail_with 'agent crashed')"
pass "4 queued dispatch" "$(sql "SELECT count(*) FROM dispatch WHERE issue_id = 'PL-3' AND run_id IS NULL")" 1
pass "4 PL-3 items while queued" "$(inbox | jq '[.items[] | select(.issue.id == "PL-3")] | length')" 0
c=$(claim); PL3=$(jq -r .run_id <<<"$c")
pass "4 PL-3 row run.state" "$(P "$SERVER_URL/api/issues" | jq -r '.issues[] | select(.issue.id == "PL-3") | .run.state')" running
pass "4 PL-3 items while running" "$(inbox | jq '[.items[] | select(.issue.id == "PL-3")] | length')" 0
finish "$PL3" "$OK"; run_once PL-3 "$OK"   # PL-3 uses up its runs so later claims reach PL-4

echo "== 5. Audience"
TOKEN=plt_second_$RANDOM$RANDOM
sql "WITH acc AS (INSERT INTO account (name, token_hash) VALUES ('Second', encode(sha256('$TOKEN'), 'hex')) RETURNING id) INSERT INTO actor (workspace_id, name, role, account_id) SELECT (SELECT workspace_id FROM actor WHERE role = 'person' LIMIT 1), 'Second', 'person', id FROM acc RETURNING id" >/dev/null
second=$(curl -s -H "Authorization: Bearer $TOKEN" "$SERVER_URL/api/inbox")
echo "  second person's inbox: $(jq -c '[.items[] | {kind, id: .issue.id}]' <<<"$second")"
pass "5 second person PL-1/PL-2 run items" "$(jq '[.items[] | select(.kind == "run")] | length' <<<"$second")" 0
pass "5 first person run items" "$(inbox | jq -c '[.items[] | select(.kind == "run") | .issue.id] | sort')" '["PL-1","PL-2","PL-3"]'
pass "5 owner of PL-1's agent" "$(sql "SELECT DISTINCT a.owner_id FROM actor a JOIN run r ON r.id = a.run_id WHERE r.issue_id = 'PL-1'")" "$(sql "SELECT id FROM actor WHERE name = 'Verify Person'")"

echo "== 6. Leaving"
before=$(inbox | jq .unread); was=$(item PL-1 | jq '.[0].unread')
jq '.repository.owner.login = "gannonh" | .repository.name = "punchlist-sandbox" | .repository.full_name = "gannonh/punchlist-sandbox" | .pull_request.head.ref = "feature/pl-1" | .pull_request.title = "One (PL-1)"' \
  "$(dirname "$C")/../../../../crates/punchlist-server/tests/fixtures/github/pull_request_closed_unmerged.json" > "$EVIDENCE/pr-closed-unmerged.json"
"$C" webhook "$RUN" pull_request "$EVIDENCE/pr-closed-unmerged.json" >/dev/null
for i in $(seq 1 50); do [ "$(sql "SELECT status FROM issue WHERE id = 'PL-1'")" = todo ] && break; sleep 0.2; done
pass "6 PL-1 status" "$(sql "SELECT status FROM issue WHERE id = 'PL-1'")" todo
pass "6 PL-1 run items" "$(item PL-1 | jq length)" 0
pass "6 PL-1 waiting items" "$(item PL-1 waiting | jq length)" 1
after=$(inbox | jq .unread)
echo "  unread $before -> $after (PL-1's run item was unread: $was; its waiting item is unread: $(item PL-1 waiting | jq '.[0].unread'))"

echo "== 7. Read"
before=$(inbox | jq .unread); seq=$(item PL-2 | jq '.[0].since_seq')
code=$(P -o /dev/null -w '%{http_code}' -X POST -H "$J" -d "{\"issue\":\"PL-2\",\"kind\":\"run\",\"since_seq\":$seq}" "$SERVER_URL/api/inbox/read")
pass "7 read answer" "$code" 204
pass "7 unread fell by 1" "$(inbox | jq .unread)" "$((before - 1))"
pass "7 inbox_read rows" "$(sql "SELECT kind || ':' || since_seq FROM inbox_read")" "run:$seq"
```
### browser-checks.cjs
```
// PL-80 live checks 8-10 in headless Chromium, recorded as one video.
const { chromium } = require(process.env.REPO + '/node_modules/.pnpm/node_modules/playwright')
const { SERVER, RT, SIGNIN, OUT, RUN3 } = process.env
const results = []
const check = (name, got, want) => {
  const ok = JSON.stringify(got) === JSON.stringify(want)
  results.push(`${ok ? 'PASS' : 'FAIL'} ${name}: ${JSON.stringify(got)}${ok ? '' : ` (want ${JSON.stringify(want)})`}`)
}
const runner = async (path, body) => {
  const r = await fetch(SERVER + path, {
    method: 'POST',
    headers: { authorization: `Bearer ${RT}`, 'content-type': 'application/json' },
    body: JSON.stringify(body),
  })
  return r.status === 204 ? null : r.json()
}
const ok = { attempt: 1, outcome: 'succeeded', duration_ms: 1 }

;(async () => {
  const browser = await chromium.launch()
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, recordVideo: { dir: OUT, size: { width: 1280, height: 800 } } })
  const page = await context.newPage()
  page.on('dialog', (d) => { results.push(`FAIL 10 a dialog opened: ${d.message()}`); void d.dismiss() })
  await page.goto(SIGNIN)
  await page.waitForURL((u) => u.pathname === '/w/PL')
  await page.waitForSelector('[data-inbox-count]')
  await page.waitForTimeout(4000)
  const count = async () => Number(await page.locator('[data-inbox-count]').textContent())

  // 8. Live header: PL-4's third run ends with the list open.
  const before = await count()
  await page.screenshot({ path: `${OUT}/8-list-before.png` })
  const t0 = Date.now()
  await runner(`/api/runs/${RUN3}/finish`, ok)
  let after = before
  while (Date.now() - t0 < 2000 && after === before) { await page.waitForTimeout(25); after = await count() }
  check('8 header count rose by 1 without a reload', after - before, 1)
  results.push(`     ${before} -> ${after} in ${Date.now() - t0} ms`)
  await page.screenshot({ path: `${OUT}/8-list-after.png` })
  await page.waitForTimeout(4000)

  // 10 setup: PL-5's third run fails with markup as its reason.
  for (const to of ['todo', 'start']) await page.request.post(`${SERVER}/api/issues/PL-5/transitions`, { data: { to } })
  const reasons = ['agent crashed', 'agent crashed', '<img src=x onerror=alert(1)>']
  for (const reason of reasons) {
    const claimed = await runner('/api/runs/claim', { wait_seconds: 0 })
    if (claimed?.issue?.id !== 'PL-5') results.push(`FAIL claim got ${claimed?.issue?.id}`)
    await runner(`/api/runs/${claimed.run_id}/finish`, { attempt: 1, outcome: 'failed', reason, duration_ms: 1 })
  }
  await page.waitForTimeout(3000)

  // 9. The screen.
  await page.keyboard.press('i')
  await page.waitForURL((u) => u.pathname === '/w/PL/inbox')
  await page.waitForSelector('[data-section=run] [data-item="PL-5"]')
  await page.waitForTimeout(3000)
  const text = (sel) => page.locator(sel).evaluateAll((els) => els.map((e) => e.textContent))
  const pl3 = '[data-section=run] [data-item="PL-3"]'
  check('9 PL-3 lines', await text(`${pl3} [data-run-line]`), ['3 runs in In Progress ended without moving it'])
  check('9 PL-3 agent', await text(`${pl3} [data-agent]`), ['Claude Code'])
  check('9 PL-3 link', await page.locator(pl3).getAttribute('href'), '/issues/PL-3')
  const pl2 = '[data-section=run] [data-item="PL-2"]'
  check('9 PL-2 lines', await text(`${pl2} [data-run-line]`), ['3 runs in In Progress ended without moving it', 'Run failed: agent crashed'])
  check('9 run items in order', await text('[data-section=run] [data-item] .font-mono'), ['PL-5', 'PL-4', 'PL-3', 'PL-2'])
  await page.screenshot({ path: `${OUT}/9-inbox-desktop.png`, fullPage: true })

  // 10. Text from outside.
  const pl5 = '[data-section=run] [data-item="PL-5"]'
  check('10 PL-5 reason as text', await text(`${pl5} [data-run-line]`), ['3 runs in In Progress ended without moving it', 'Run failed: <img src=x onerror=alert(1)>'])
  check('10 img elements in the item', await page.locator(`${pl5} img`).count(), 0)
  await page.locator(pl5).scrollIntoViewIfNeeded()
  await page.screenshot({ path: `${OUT}/10-markup-as-text.png` })

  await page.setViewportSize({ width: 390, height: 800 })
  await page.waitForTimeout(2000)
  check('9 no overflow at 390 px', await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), true)
  await page.screenshot({ path: `${OUT}/9-inbox-390.png`, fullPage: true })
  await page.setViewportSize({ width: 1280, height: 800 })
  await page.waitForTimeout(2000)

  // Opening an item reads it and goes to the issue.
  const unread = await count()
  await page.locator(pl3).focus()
  await page.waitForTimeout(1500)
  await page.keyboard.press('Enter')
  await page.waitForURL((u) => u.pathname === '/issues/PL-3')
  check('9 Enter opens the issue', new URL(page.url()).pathname, '/issues/PL-3')
  await page.waitForTimeout(4000)
  await page.keyboard.press('Escape')
  await page.waitForURL((u) => u.pathname === '/w/PL')
  await page.waitForTimeout(2000)
  check('9 opening PL-3 read it', await count(), unread - 1)
  await page.waitForTimeout(3000)
  await context.close()
  await browser.close()
  console.log(results.join('\n'))
})().catch((e) => { console.error(e); process.exit(1) })
```
### perf.sh
```
#!/usr/bin/env bash
# Seeds 1000 issues (100 in Todo, 50 with three ended runs) and times GET /api/inbox and /api/issues, median of 5.
set -uo pipefail
RUN=$1
source /tmp/punchlist-verify/$RUN/run.env
TOK=$(grep '^token' /tmp/punchlist-verify/$RUN/config.toml | cut -d'"' -f2)
J='content-type: application/json'
P() { curl -s -H "Authorization: Bearer $TOK" -H "$J" "$@"; }
RT=$(P -X POST -d '{"name":"perf","agents":["claude-code"]}' "$SERVER_URL/api/runners" | jq -r .token)
R() { curl -s -H "Authorization: Bearer $RT" -H "$J" "$@"; }
base=$(psql "$DATABASE_URL" -XtAc "SELECT count(*) FROM issue")
for i in $(seq 1 1000); do P -o /dev/null -d "{\"title\":\"Perf $i\",\"body\":\"Seeded issue $i\"}" "$SERVER_URL/api/issues"; done
mv() { P -o /dev/null -d "{\"to\":\"$2\"}" "$SERVER_URL/api/issues/$1/transitions"; }
for i in $(seq 1 150); do mv "PL-$((base + i))" todo; done
for i in $(seq 101 150); do
  id="PL-$((base + i))"; mv "$id" start
  for k in 1 2 3; do
    run=$(R -d '{"wait_seconds":0}' "$SERVER_URL/api/runs/claim" | jq -r .run_id)
    R -o /dev/null -d '{"attempt":1,"outcome":"succeeded","duration_ms":1}' "$SERVER_URL/api/runs/$run/finish"
  done
done
echo "issues $(psql "$DATABASE_URL" -XtAc "SELECT count(*) FROM issue"), todo $(psql "$DATABASE_URL" -XtAc "SELECT count(*) FROM issue WHERE status = 'todo'"), runs $(psql "$DATABASE_URL" -XtAc "SELECT count(*) FROM run")"
median() { sort -n | sed -n 3p; }
for path in inbox issues; do
  P -o /dev/null "$SERVER_URL/api/$path"   # warm
  times=$(for k in 1 2 3 4 5; do P -o /dev/null -w '%{time_total}\n' "$SERVER_URL/api/$path"; done)
  echo "GET /api/$path: $(echo $times) median $(echo "$times" | median) s"
done
echo "inbox items: $(P "$SERVER_URL/api/inbox" | jq -c '[.items | length, ([.[] | select(.kind == "run")] | length)]')"
```
