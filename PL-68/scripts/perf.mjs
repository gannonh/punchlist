// Check 10 (and check 1 on main): node perf.mjs <main|branch> <SERVER_URL> <SIGN_IN_URL>
import { writeFileSync } from 'node:fs'
import { chromium } from '/home/gannonh/.local/share/punchlist/worktrees/worktrees/gannonh/punchlist/PL-68/node_modules/.pnpm/node_modules/playwright/index.mjs'

const [mode, server, signIn] = process.argv.slice(2)
const OUT = '/home/gannonh/.cache/pl68-live/evidence'
const browser = await chromium.launch()
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } })
await page.goto(signIn)
await page.waitForURL((u) => u.pathname === '/')
const result = { mode }
await page.waitForFunction(() => document.querySelectorAll('[data-issue]').length === 500)
if (mode === 'branch') await page.locator('[data-pull-request-count]').waitFor()
const median = (xs) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)]

result.check1 = {
  link: await page.evaluate(() => document.querySelector('a[href="/pull-requests"]') !== null),
  route: await page.evaluate(() => fetch('/api/pull-requests').then((r) => r.status)),
}

const loads = []
for (let i = 0; i < 16; i++) {
  await page.goto(`${server}/`)
  await page.waitForFunction(() => document.querySelectorAll('[data-issue]').length === 500)
  if (mode === 'branch') await page.locator('[data-pull-request-count]').waitFor()
  await page.waitForTimeout(300)
  loads.push(
    await page.evaluate(() => {
      const render = performance.getEntriesByName('pl:render').find((m) => m.detail.rows === 500)
      const res = performance.getEntriesByType('resource')
      const issues = res.find((r) => new URL(r.name).pathname === '/api/issues')
      const prs = res.find((r) => new URL(r.name).pathname === '/api/pull-requests')
      return {
        render: render?.duration,
        issuesResponseEnd: issues?.responseEnd,
        pullRequestsStart: prs?.startTime ?? null,
        count: document.querySelector('[data-pull-request-count]')?.textContent ?? null,
      }
    }),
  )
}
result.loads = loads.slice(1)
result.listRenderMedian = median(result.loads.map((l) => l.render))
result.prAfterIssues = result.loads.every((l) => l.pullRequestsStart === null || l.pullRequestsStart > l.issuesResponseEnd)

if (mode === 'branch') {
  const pages = []
  for (let i = 0; i < 6; i++) {
    await page.goto(`${server}/pull-requests`)
    await page.locator('[data-section]').first().waitFor()
    pages.push(
      await page.evaluate(() => {
        const m = performance.getEntriesByName('pl:render').filter((e) => e.detail.layout === 'pull-requests')[0]
        return { duration: m.duration, rows: m.detail.rows }
      }),
    )
  }
  result.pageRenders = pages.slice(1)
  result.pageRenderMedian = median(result.pageRenders.map((p) => p.duration))
  await page.goto(`${server}/`)
  await page.waitForFunction(() => document.querySelectorAll('[data-issue]').length === 500)
  await page.locator('[data-pull-request-count]').waitFor()
  await page.screenshot({ path: `${OUT}/10-list-header-with-count.png` })
}
await browser.close()
writeFileSync(`${OUT}/perf-${mode}.json`, JSON.stringify(result, null, 2))
console.log(JSON.stringify(result, null, 1))
