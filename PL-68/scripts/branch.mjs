// Live checks 1 (branch half) and 3 to 9 of PL-68 against the run in ~/.cache/pl68-live/env.sh.
import { execFileSync } from 'node:child_process'
import { mkdirSync, writeFileSync } from 'node:fs'
import { chromium } from '/home/gannonh/.local/share/punchlist/worktrees/worktrees/gannonh/punchlist/PL-68/node_modules/.pnpm/node_modules/playwright/index.mjs'

const OUT = '/home/gannonh/.cache/pl68-live/evidence'
mkdirSync(OUT, { recursive: true })
const sh = (cmd) => execFileSync('bash', ['-c', `source ~/.cache/pl68-live/env.sh >/dev/null; ${cmd}`], { encoding: 'utf8' })
const results = []
const log = []
const check = (name, ok, detail) => {
  results.push({ name, ok, detail })
  const line = `${ok ? 'PASS' : 'FAIL'} ${name} ${JSON.stringify(detail)}`
  log.push(line)
  console.log(line)
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
async function within(ms, probe) {
  const start = Date.now()
  for (;;) {
    const value = await probe()
    if (value) return { ok: true, ms: Date.now() - start }
    if (Date.now() - start > ms) return { ok: false, ms: Date.now() - start }
    await sleep(50)
  }
}

const server = sh('echo $SERVER_URL').trim()
const signIn = sh('$C sign-in-url "$RUN"').trim()
const browser = await chromium.launch()
const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, recordVideo: { dir: OUT, size: { width: 1280, height: 800 } } })
const page = await context.newPage()
const path = () => page.evaluate(() => location.pathname)
const text = (sel) => page.locator(sel).innerText()
const has = (sel) => page.evaluate((s) => document.querySelector(s) !== null, sel)

await page.goto(signIn)
await page.waitForURL((u) => u.pathname === '/')
await page.locator('[data-issue]').first().waitFor()
await page.locator('[data-pull-request-count]').waitFor()

// 1. Branch half.
{
  const link = await has('a[href="/pull-requests"]')
  const status = await page.evaluate(() => fetch('/api/pull-requests').then((r) => r.status))
  await page.goto(`${server}/pull-requests`)
  await page.locator('[data-section]').first().waitFor()
  const sections = await page.locator('[data-section]').count()
  check('1 branch: link, route, sections', link && status === 200 && sections > 0, { link, status, sections })
}

// 3. Entry and keys.
{
  const seen = {}
  for (const [name, url] of [['list', '/'], ['board', '/?layout=board']]) {
    await page.goto(server + url)
    await page.locator('[data-pull-request-count]').waitFor()
    const count = await text('[data-pull-request-count]')
    await page.getByRole('link', { name: /Pull requests/ }).click()
    await page.waitForURL((u) => u.pathname === '/pull-requests')
    seen[name] = { path: await path(), count }
  }
  await page.goto(server + '/')
  await page.locator('[data-issue]').first().waitFor()
  await page.keyboard.press('g')
  await page.keyboard.press('p')
  await page.waitForURL((u) => u.pathname === '/pull-requests', { timeout: 2000 }).catch(() => {})
  seen.chord = await path()
  await page.goto(server + '/')
  await page.locator('[data-issue]').first().waitFor()
  await page.getByLabel('Filter issues').focus()
  await page.keyboard.type('gp')
  await sleep(300)
  seen.typed = { path: await path(), value: await page.getByLabel('Filter issues').inputValue() }
  await page.goto(server + '/pull-requests')
  await page.locator('[data-section]').first().waitFor()
  await page.keyboard.press('Escape')
  await page.waitForURL((u) => u.pathname === '/', { timeout: 2000 }).catch(() => {})
  seen.escape = await path()
  check(
    '3 entry and keys',
    seen.list.path === '/pull-requests' && seen.list.count === '4' && seen.board.path === '/pull-requests' &&
      seen.board.count === '4' && seen.chord === '/pull-requests' && seen.typed.path === '/' &&
      seen.typed.value === 'gp' && seen.escape === '/',
    seen,
  )
}

// 4. Sections and rows.
{
  await page.goto(server + '/pull-requests')
  await page.locator('[data-section]').first().waitFor()
  await page.screenshot({ path: `${OUT}/04-pull-requests-desktop.png`, fullPage: true })
  const order = await page.evaluate(() => [...document.querySelectorAll('[data-section]')].map((s) => s.dataset.section))
  const row7 = await text('[data-pr-row="7"]')
  const merge7 = await text('[data-pr-row="7"] [data-merge-state]')
  const state7 = sh(`$C sql "$RUN" "SELECT merge_state FROM pull_request WHERE number = 7" | sed -n 3p`).trim()
  const row9 = await text('[data-pr-row="9"]')
  await page.setViewportSize({ width: 390, height: 844 })
  await sleep(200)
  const fits = await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth)
  await page.screenshot({ path: `${OUT}/04-pull-requests-phone.png`, fullPage: true })
  await page.setViewportSize({ width: 1280, height: 800 })
  const wanted = ['#7', 'PL-1', 'In Progress', 'checks 0/0', '0 open threads']
  check(
    '4 sections and rows',
    JSON.stringify(order) === '["waits-on-merge","in-review","draft","names-no-issue"]' &&
      wanted.every((w) => row7.includes(w)) && state7 === 'unknown' && merge7 === 'Merge state not known yet' &&
      row9.includes('names no issue') && fits,
    { order, row7, state7, merge7, row9, fits },
  )
}

// 5. Waits on a merge.
{
  const inside = await has('[data-section="waits-on-merge"] [data-pr-row="14"]')
  const row14 = await text('[data-pr-row="14"]')
  const merge = await page.locator('[data-pr-row="14"] a', { hasText: 'Merge on GitHub' }).getAttribute('href')
  const row7 = await text('[data-pr-row="7"]')
  const notice = 'Waits on a merge on GitHub. PL-3 moves to Done when GitHub reports the merge.'
  await page.locator('[data-pr-row="14"]').screenshot({ path: `${OUT}/05-waits-on-merge-row.png` })
  sh('pr closed pull_request_closed_merged 14 feature/pl-3-d $(sha 14) "Live three" false')
  const gone = await within(2000, async () => !(await has('[data-pr-row="14"]')))
  const status = sh(`$C sql "$RUN" "SELECT status FROM issue WHERE id = 'PL-3'" | sed -n 3p`).trim()
  check(
    '5 waits on a merge',
    inside && row14.replace(/\s+/g, ' ').includes(notice) &&
      merge === 'https://github.com/gannonh/punchlist-sandbox/pull/14' && !row7.includes('Waits on a merge') &&
      gone.ok && status === 'done',
    { inside, row14, merge, row7HasNotice: row7.includes('Waits on a merge'), goneMs: gone.ms, status },
  )
}

// 6. Live: appears and moves.
{
  sh('pr opened pull_request_opened 11 feature/pl-3-c $(sha 11) "Live eleven" false')
  const review = await within(2000, () => has('[data-section="in-review"] [data-pr-row="11"]'))
  await page.screenshot({ path: `${OUT}/06-pull-request-11-in-review.png`, fullPage: true })
  sh('pr converted_to_draft pull_request_opened 11 feature/pl-3-c $(sha 11) "Live eleven" true')
  const draft = await within(2000, () => has('[data-section="draft"] [data-pr-row="11"]'))
  sh('pr closed pull_request_closed_unmerged 11 feature/pl-3-c $(sha 11) "Live eleven" false')
  const gone = await within(2000, async () => !(await has('[data-pr-row="11"]')))
  const events = sh(`$C sql "$RUN" "SELECT seq, issue_id, kind FROM event WHERE kind = 'pull_request' ORDER BY seq"`)
  check('6 live: appears, moves, leaves', review.ok && draft.ok && gone.ok, {
    reviewMs: review.ms,
    draftMs: draft.ms,
    goneMs: gone.ms,
    events,
  })
}

// 7. Live: checks, threads.
{
  sh('check 7 feature/pl-1-a $(sha 7) success')
  sh('thread unresolved 7 feature/pl-1-a $(sha 7) "Live one"')
  const seen = await within(2000, async () => {
    const t = await text('[data-pr-row="7"]')
    return t.includes('checks 1/1') && t.includes('1 open thread')
  })
  const row7 = await text('[data-pr-row="7"]')
  const rows = sh(`$C sql "$RUN" "SELECT checks_passed, checks_total, open_threads FROM pull_request WHERE number = 7"`)
  check('7 live: checks and threads', seen.ok && /\b1 \| +1 \| +1\b/.test(rows), { ms: seen.ms, row7, rows })
}

// 8. Unlinked.
{
  const unlinked = await page.evaluate(() =>
    [...document.querySelectorAll('[data-section="names-no-issue"] [data-pr-row]')].map((r) => Number(r.dataset.prRow)),
  )
  const cli = sh('$C pl "$RUN" pr unlinked')
  await page.evaluate(() => {
    window.__opens = []
    window.open = (...args) => {
      window.__opens.push(args)
      return null
    }
  })
  await page.locator('[data-pr-row="9"]').focus()
  await page.keyboard.press('Enter')
  const opens = await page.evaluate(() => window.__opens)
  const stayed = await path()
  sh('pr opened pull_request_opened 12 stray-two $(sha 12) "Another stray" false')
  await sleep(2500)
  const before = await has('[data-pr-row="12"]')
  sh('$C pl "$RUN" issue comment PL-1 --body ping')
  await sleep(2000)
  const afterComment = await has('[data-pr-row="12"]')
  await page.reload()
  await page.locator('[data-section]').first().waitFor()
  const afterReload = await has('[data-pr-row="12"]')
  check(
    '8 unlinked',
    JSON.stringify(unlinked) === '[9]' && cli === '#9  open  stray  Bump deps\n' && opens.length === 1 &&
      opens[0][0] === 'https://github.com/gannonh/punchlist-sandbox/pull/9' && opens[0][2] === 'noopener' &&
      stayed === '/pull-requests' && !before && afterReload,
    { unlinked, cli, opens, stayed, before, afterComment, afterReload },
  )
}

// 9. Links and plain text.
{
  const link = page.locator('[data-pr-row="7"] a', { hasText: 'Open on GitHub' })
  const attrs = { href: await link.getAttribute('href'), target: await link.getAttribute('target'), rel: await link.getAttribute('rel') }
  await page.locator('[data-pr-row="7"]').focus()
  await page.keyboard.press('Enter')
  await page.waitForURL((u) => u.pathname === '/issues/PL-1', { timeout: 2000 }).catch(() => {})
  const opened = await path()
  const title = '<img src=x onerror="window.__x=1">'
  sh(`pr opened pull_request_opened 13 xss $(sha 13) '${title}' false`)
  sh(`$C sql "$RUN" "UPDATE pull_request SET url = 'javascript:alert(1)' WHERE number = 13"`)
  await page.goto(server + '/pull-requests')
  await page.locator('[data-pr-row="13"]').waitFor()
  const row13 = await text('[data-pr-row="13"]')
  const img = await page.locator('[data-pr-row="13"] img').count()
  const x = await page.evaluate(() => typeof window.__x)
  const gh = await page.locator('[data-pr-row="13"] a').count()
  await page.screenshot({ path: `${OUT}/09-plain-text-title.png`, fullPage: true })
  check(
    '9 links and plain text',
    attrs.href === 'https://github.com/gannonh/punchlist-sandbox/pull/7' && attrs.target === '_blank' &&
      attrs.rel.includes('noopener') && opened === '/issues/PL-1' && row13.includes(title) && img === 0 &&
      x === 'undefined' && gh === 0,
    { attrs, opened, row13, img, x, links: gh },
  )
}

await context.close()
await browser.close()
writeFileSync(`${OUT}/branch-results.json`, JSON.stringify(results, null, 2))
writeFileSync(`${OUT}/branch-checks.txt`, log.join('\n') + '\n')
process.exit(results.every((r) => r.ok) ? 0 : 1)
