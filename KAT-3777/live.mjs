// KAT-3777 live checks: drive the web app headless, perform each action, and time the row's change.
// node live.mjs <repo of the control script> <run id> <mode: main | branch | leases | runner>
import { createRequire } from 'node:module'
import { execFileSync } from 'node:child_process'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'

const WORKTREE = '/home/gannonh/dev/punchlist/.claude/worktrees/agent-ac07334af91f2b292'
const { chromium } = createRequire(`${WORKTREE}/package.json`)('./node_modules/.pnpm/node_modules/playwright')
const [repo, runId, mode] = process.argv.slice(2)
const env = Object.fromEntries(
  readFileSync(`/tmp/punchlist-verify/${runId}/run.env`, 'utf8')
    .trim()
    .split('\n')
    .map((line) => line.split(/=(.*)/s).slice(0, 2)),
)
const person = /^token = "(.*)"$/m.exec(readFileSync(env.CONFIG, 'utf8'))[1]
const fixtures = `${WORKTREE}/crates/punchlist-server/tests/fixtures/github`
const evidence = `${env.EVIDENCE}/live-${mode}`
mkdirSync(`${evidence}/shots`, { recursive: true })
const results = []
const LIMIT = 2000

const C = (...args) =>
  execFileSync(`${repo}/.claude/skills/verify-punchlist/bin/control-punchlist`, args, {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  })
const sql = (query) => C('sql', runId, query)

async function api(method, path, token, body) {
  const response = await fetch(`${env.SERVER_URL}${path}`, {
    method,
    headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const text = await response.text()
  if (!response.ok) throw new Error(`${method} ${path} answered ${response.status}: ${text}`)
  return text ? JSON.parse(text) : null
}

let deliveries = 0
/** A fixture delivery through `$C webhook`, for the sandbox repository; returns when the server answered. */
function webhook(event, fixture, edit) {
  const payload = JSON.parse(readFileSync(`${fixtures}/${fixture}.json`, 'utf8'))
  payload.repository.name = 'punchlist-sandbox'
  edit(payload)
  const file = `${evidence}/delivery-${String(++deliveries).padStart(2, '0')}-${fixture}.json`
  writeFileSync(file, JSON.stringify(payload, null, 2))
  const answer = C('webhook', runId, event, file).trim()
  if (answer !== 'HTTP 202') throw new Error(`webhook answered ${answer}`)
  return answer
}
const pullRequest = (issue) => (p) => {
  p.number = 7
  p.pull_request.number = 7
  p.pull_request.head.ref = `feature/${issue.toLowerCase()}-live`
}
const checkRun = (id, status, conclusion) => (p) => {
  p.check_run.id = id
  p.check_run.status = status
  p.check_run.conclusion = conclusion
}

const browser = await chromium.launch()
const context = await browser.newContext({
  viewport: { width: 1280, height: 800 },
  recordVideo: { dir: `${evidence}/video`, size: { width: 1280, height: 800 } },
})
const page = await context.newPage()
const cell = (issue, selector) => `[data-issue="${issue}"] ${selector}`

/** The page's clock when `test(element)` first holds for the issue's cell, polled every 10 ms; null after `wait` ms. */
async function when(issue, selector, test, wait = 5000) {
  try {
    const handle = await page.waitForFunction(
      ({ query, test }) => {
        const element = document.querySelector(query)
        // eslint-disable-next-line no-new-func
        return element && new Function('e', `return ${test}`)(element) ? Date.now() : false
      },
      { query: cell(issue, selector), test },
      { polling: 10, timeout: wait },
    )
    return await handle.jsonValue()
  } catch {
    return null
  }
}
const read = (issue, selector, what = 'textContent') =>
  page.evaluate(({ query, what }) => {
    const element = document.querySelector(query)
    if (!element) return null
    return what === 'textContent' ? element.textContent.trim() : element.getAttribute(what)
  }, { query: cell(issue, selector), what })

/** A caption over the page for the video: which action ran, and how long the row took. */
const caption = (text) =>
  page.evaluate((text) => {
    let box = document.getElementById('live-check-caption')
    if (!box) {
      box = document.createElement('div')
      box.id = 'live-check-caption'
      box.style.cssText =
        'position:fixed;left:16px;right:16px;bottom:56px;padding:12px 16px;border-radius:8px;background:#111;color:#fff;font:14px/1.4 ui-monospace,monospace;z-index:9999'
      document.body.append(box)
    }
    box.textContent = text
  }, text)

/** Times one action against the row change it should make. */
async function timed(check, step, act, issue, selector, test, observe) {
  await caption(`Check ${check}: ${step}…`)
  const answer = await act()
  const answeredAt = Date.now()
  const domAt = await when(issue, selector, test)
  const observed = await read(issue, selector, observe)
  const delta = domAt === null ? null : domAt - answeredAt
  const pass = delta !== null && delta <= LIMIT
  results.push({ check, step, answer, answered_at: answeredAt, dom_at: domAt, delta_ms: delta, observed, pass })
  console.log(`${pass ? 'PASS' : 'FAIL'}  check ${check}  ${step}: ${delta ?? '>5000'} ms  (${observed})`)
  await caption(`Check ${check}: ${step} → ${observed} in ${delta ?? '>5000'} ms, no reload`)
  await page.screenshot({ path: `${evidence}/shots/check-${check}-${step.replace(/\W+/g, '-')}.png` })
  await page.waitForTimeout(2500) // for the video
}

/** Asserts the row has not changed `wait` ms after the action: the `main` half of check 1. */
async function unchanged(check, step, act, issue, selector, test, observe, wait = 5000) {
  const answer = await act()
  const domAt = await when(issue, selector, test, wait)
  const observed = await read(issue, selector, observe)
  const pass = domAt === null
  results.push({ check, step, answer, changed_within_ms: domAt, observed, pass })
  console.log(`${pass ? 'PASS' : 'FAIL'}  check ${check}  ${step}: unchanged after ${wait} ms  (${observed})`)
  await page.screenshot({ path: `${evidence}/shots/check-${check}-${step.replace(/\W+/g, '-')}.png` })
}

async function signIn() {
  const url = C('sign-in-url', runId).trim()
  await page.goto(url)
  await page.waitForURL((u) => u.pathname === '/')
  await page.evaluate(() => sessionStorage.setItem('loaded', String(performance.timeOrigin)))
}
const sameDocument = () =>
  page.evaluate(() => sessionStorage.getItem('loaded') === String(performance.timeOrigin))

/** PL-n moved to Start by the person with `pl`, then claimed through the API by a runner that heartbeats. */
async function claimedIssue(title) {
  const id = C('pl', runId, 'issue', 'create', '--title', title, '--body', 'A live check of the open list.').trim()
  for (const to of ['todo', 'start']) C('pl', runId, 'issue', 'move', id, to)
  const runner = await api('POST', '/api/runners', person, { name: `api-${id.toLowerCase()}`, agents: ['claude-code'] })
  const claim = await api('POST', '/api/runs/claim', runner.token, {})
  if (claim.issue.id !== id) throw new Error(`claimed ${claim.issue.id}, not ${id}`)
  return { id, runner, claim }
}
function heartbeat(runner, held) {
  const beat = () =>
    api('POST', `/api/runners/${runner.runner.id}/heartbeat`, runner.token, { held: held() }).catch((error) =>
      console.error(`heartbeat: ${error.message}`),
    )
  return setInterval(beat, 5000)
}

const running = "e.dataset.run === 'running'"
const text = (value) => `e.textContent.trim() === ${JSON.stringify(value)}`
const title = (value) => `e.title === ${JSON.stringify(value)}`
const gates = (value) => `e.dataset.gates === ${JSON.stringify(value)}`

if (mode === 'main' || mode === 'branch') {
  const { id, runner, claim } = await claimedIssue('Updated live')
  let held = [{ run_id: claim.run_id, attempt: claim.attempt }]
  const beating = heartbeat(runner, () => held)
  await signIn()
  if ((await when(id, '[data-run]', running)) === null) throw new Error('the claimed row is not Running')
  await page.screenshot({ path: `${evidence}/shots/check-1-start.png` })
  await page.waitForTimeout(1500)
  const finish = () =>
    api('POST', `/api/runs/${claim.run_id}/finish`, runner.token, {
      attempt: claim.attempt,
      outcome: 'failed',
      reason: 'live check',
      duration_ms: 1,
    }).then((run) => {
      held = []
      return `run ${run.outcome}`
    })
  const opened = () => webhook('pull_request', 'pull_request_opened', pullRequest(id))
  if (mode === 'main') {
    await unchanged(1, 'finish the run', finish, id, '[data-run]', "e.dataset.run !== 'running'")
    await unchanged(1, 'deliver opened', opened, id, '[data-pr]', "e.dataset.pr !== 'none'")
  } else {
    await timed(1, 'finish the run', finish, id, '[data-run]', text('Queued'))
    await timed(1, 'deliver opened', opened, id, '[data-pr]', text('#7 draft'))

    let second
    await timed(2, 'claim the waiting dispatch', async () => {
      second = await api('POST', '/api/runs/claim', runner.token, {})
      held = [{ run_id: second.run_id, attempt: second.attempt }]
      return `${second.issue.id} run ${second.run_id} attempt ${second.attempt}`
    }, id, '[data-run]', running)

    const ready = () => webhook('pull_request', 'pull_request_ready_for_review', pullRequest(id))
    await timed(4, 'deliver ready_for_review', ready, id, '[data-pr]', text('#7 open'))
    results.push({ check: 4, step: 'gates after ready', observed: await read(id, '[data-gates]'), pass: (await read(id, '[data-gates]', 'data-gates')) === '3/3' })

    await timed(5, 'agent requests Agent Review', async () => {
      const moved = await api('POST', `/api/issues/${id}/transitions`, second.agent_token, { to: 'agent_review' })
      return `${moved.event.detail.from} -> ${moved.event.detail.to}`
    }, id, '[data-gates]', gates('2/4'), 'title')
    const sha = '6dcb09b5b57875f334f61aebed695e2e4193db5e'
    await timed(5, 'deliver check_run created (queued)', () =>
      webhook('check_run', 'check_run_completed', (p) => {
        checkRun(7001, 'queued', null)(p)
        p.action = 'created'
        p.check_run.head_sha = sha
      }), id, '[data-pr]', title('PR #7 open, checks pending'), 'title')
    await timed(5, 'deliver check_run completed (success)', () =>
      webhook('check_run', 'check_run_completed', checkRun(7001, 'completed', 'success')), id, '[data-gates]', gates('3/4'), 'title')
    results.push({ check: 5, step: 'checks after success', observed: await read(id, '[data-pr]', 'title'), pass: (await read(id, '[data-pr]', 'title')) === 'PR #7 open, checks passing' })

    writeFileSync(`${evidence}/shot.png`, Buffer.from('89504e470d0a1a0a', 'hex'))
    await timed(6, 'pl proof attach', () =>
      C('pl', runId, 'proof', 'attach', id, `${evidence}/shot.png`, '--title', 'Screen').trim(),
      id, '[data-gates]', gates('4/4'), 'title')

    const before = Number(/\n\s*(\d+)\s*\n/.exec(sql('SELECT max(seq) FROM event'))[1])
    results.push({ check: 8, step: 'seq before the burst', observed: before, pass: true })
    await timed(8, 'deliver check_run 7100 created (queued)', () =>
      webhook('check_run', 'check_run_completed', (p) => {
        checkRun(7100, 'queued', null)(p)
        p.action = 'created'
        p.check_run.name = 'gate'
      }), id, '[data-pr]', title('PR #7 open, checks pending'), 'title')
    const burstStart = Date.now()
    // Each its own check, so none supersedes `gate` (the newest run of a name in a suite counts).
    for (let n = 1; n <= 20; n++)
      webhook('check_run', 'check_run_completed', (p) => {
        checkRun(7100 + n, 'completed', 'success')(p)
        p.check_run.name = `check-${n}`
      })
    const burstEnd = Date.now()
    results.push({ check: 8, step: '20 check_run completed in a burst', burst_ms: burstEnd - burstStart, observed: await read(id, '[data-pr]', 'title'), pass: (await read(id, '[data-pr]', 'title')) === 'PR #7 open, checks pending' })
    await timed(8, 'complete check_run 7100 (success)', () =>
      webhook('check_run', 'check_run_completed', (p) => {
        checkRun(7100, 'completed', 'success')(p)
        p.check_run.name = 'gate'
      }),
      id, '[data-pr]', title('PR #7 open, checks passing'), 'title')
    writeFileSync(`${evidence}/seq-before-burst.txt`, String(before))

    await page.keyboard.press('2')
    await page.waitForSelector(`[data-column-section] [data-issue="${id}"]`)
    results.push({ check: 10, step: 'one card for the issue on the board', observed: await page.locator(`[data-issue="${id}"]`).count(), pass: (await page.locator(`[data-issue="${id}"]`).count()) === 1 })
    await page.waitForTimeout(1000)
    await timed(10, 'board: deliver check_run completed (failure)', () =>
      webhook('check_run', 'check_run_completed', checkRun(7200, 'completed', 'failure')),
      id, '[data-pr]', title('PR #7 open, checks failing'), 'title')
  }
  results.push({ check: 1, step: 'no reload', observed: await sameDocument(), pass: await sameDocument() })
  clearInterval(beating)
}

if (mode === 'leases') {
  const { id, runner } = await claimedIssue('Lease lapses')
  await signIn()
  if ((await when(id, '[data-run]', running)) === null) throw new Error('the claimed row is not Running')
  const lapsed = (attempt) => {
    for (;;) {
      const out = execFileSync('psql', [env.DATABASE_URL, '-tAc', `SELECT count(*) FROM attempt WHERE issue_id = '${id}' AND attempt = ${attempt} AND state = 'failed'`], { encoding: 'utf8' })
      if (out.trim() === '1') return
      execFileSync('sleep', ['1'])
    }
  }
  console.log('waiting for the first lease to lapse (30 s lease, 5 s sweep)')
  lapsed(1)
  await timed(7, 'claim attempt 2', async () => {
    const claim = await api('POST', '/api/runs/claim', runner.token, {})
    return `run ${claim.run_id} attempt ${claim.attempt}`
  }, id, '[data-run]', running)
  console.log('waiting for the second lease to lapse')
  const queued = when(id, '[data-run]', text('Queued'), 60_000)
  lapsed(2)
  const domAt = await queued
  const finished = /\n\s*(\d+)\s*\n/.exec(
    sql(`SELECT (extract(epoch FROM finished_at) * 1000)::bigint FROM attempt WHERE issue_id = '${id}' AND attempt = 2`),
  )[1]
  const delta = domAt - Number(finished)
  results.push({ check: 7, step: 'second lapse shows Queued', attempt_finished_at: Number(finished), dom_at: domAt, delta_ms: delta, observed: await read(id, '[data-run]'), pass: delta <= LIMIT })
  console.log(`${delta <= LIMIT ? 'PASS' : 'FAIL'}  check 7  second lapse: ${delta} ms after finished_at`)
  await page.screenshot({ path: `${evidence}/shots/check-7-queued.png` })
}

if (mode === 'runner') {
  const id = C('pl', runId, 'issue', 'create', '--title', 'Live check: change nothing', '--body',
    'This issue is a live check of the issue list. Do not change, commit or push anything. Stop at once.').trim()
  C('pl', runId, 'issue', 'move', id, 'todo')
  await signIn()
  await page.waitForSelector(`[data-group="todo"] [data-issue="${id}"]`)
  await page.waitForTimeout(1000)
  const domAt = await (async () => {
    C('pl', runId, 'issue', 'move', id, 'start')
    return when(id, '[data-run]', running, 15_000)
  })()
  const started = /\n\s*(\d+)\s*\n/.exec(
    sql(`SELECT (extract(epoch FROM started_at) * 1000)::bigint FROM attempt WHERE issue_id = '${id}' AND attempt = 1`),
  )[1]
  const delta = domAt - Number(started)
  const group = await page.evaluate((id) => document.querySelector(`[data-issue="${id}"]`)?.closest('[data-group]')?.dataset.group, id)
  results.push({ check: 3, step: 'a real runner claims from Start', attempt_started_at: Number(started), dom_at: domAt, delta_ms: delta, observed: `${await read(id, '[data-run]')} in ${group}`, pass: delta <= LIMIT && group === 'in_progress' })
  console.log(`${delta <= LIMIT && group === 'in_progress' ? 'PASS' : 'FAIL'}  check 3  claim: ${delta} ms after started_at, in ${group}`)
  await page.screenshot({ path: `${evidence}/shots/check-3-running.png` })
}

await context.close()
await browser.close()
writeFileSync(`${evidence}/results.json`, JSON.stringify(results, null, 2))
const failed = results.filter((r) => !r.pass)
console.log(`${results.length - failed.length} of ${results.length} steps pass`)
process.exit(failed.length ? 1 : 0)
