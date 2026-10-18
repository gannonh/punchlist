import { execFileSync } from 'node:child_process'
import { createHmac, randomUUID } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

// The person token `bootstrap` wrote into the run's `pl` config.
function personToken(): string {
  const config = process.env.E2E_CONFIG
  if (!config) throw new Error('E2E_CONFIG is not set; run `pnpm e2e`')
  const token = /^token = "(.*)"$/m.exec(readFileSync(config, 'utf8'))?.[1]
  if (!token) throw new Error(`no token in ${config}`)
  return token
}

const bearer = (token: string) => ({ Authorization: `Bearer ${token}` })

async function ok<T>(response: Awaited<ReturnType<APIRequestContext['post']>>): Promise<T> {
  expect(response.ok(), `${response.url()} answered ${String(response.status())}: ${await response.text()}`).toBe(true)
  return (await response.json()) as T
}

interface Claimed {
  id: string
  runner: string
  claim: { run_id: string; attempt: number }
}

/** An issue in In Progress without a model: moved to Start by the person, then claimed by a runner through the API. */
async function claimedInProgress(request: APIRequestContext, title: string): Promise<Claimed> {
  const person = bearer(personToken())
  const { row } = await ok<{ row: { issue: { id: string } } }>(
    await request.post('/api/issues', { headers: person, data: { title, body: '' } }),
  )
  const id = row.issue.id
  for (const to of ['todo', 'start'])
    await ok(await request.post(`/api/issues/${id}/transitions`, { headers: person, data: { to } }))
  const runner = await ok<{ token: string }>(
    await request.post('/api/runners', { headers: person, data: { name: `e2e-${id}`, agents: ['claude-code'] } }),
  )
  // Claims go in issue order, so an earlier test's attempt whose lease lapsed comes first: this runner takes it
  // too, and claims again.
  let claim: Claimed['claim'] & { issue: { id: string; status: string } }
  do {
    claim = await ok(await request.post('/api/runs/claim', { headers: bearer(runner.token), data: {} }))
  } while (claim.issue.id !== id)
  expect(claim.issue).toMatchObject({ id, status: 'in_progress' })
  return { id, runner: runner.token, claim }
}

const issueInProgress = async (request: APIRequestContext, title: string) =>
  (await claimedInProgress(request, title)).id

const fixture = (name: string): unknown =>
  JSON.parse(
    readFileSync(new URL(`../../crates/punchlist-server/tests/fixtures/github/${name}.json`, import.meta.url), 'utf8'),
  )

/** A delivery to the webhook on the run's repository, signed as GitHub signs one. */
async function deliver(request: APIRequestContext, event: string, payload: { repository: { name: string } }) {
  const secret = process.env.E2E_WEBHOOK_SECRET_FILE
  if (!secret) throw new Error('E2E_WEBHOOK_SECRET_FILE is not set; run `pnpm e2e`')
  payload.repository.name = 'punchlist-sandbox'
  const body = JSON.stringify(payload)
  const signature = createHmac('sha256', readFileSync(secret, 'utf8')).update(body).digest('hex')
  const response = await request.post('/api/github/webhook', {
    headers: {
      'content-type': 'application/json',
      'x-github-event': event,
      'x-github-delivery': randomUUID(),
      'x-hub-signature-256': `sha256=${signature}`,
    },
    data: body,
  })
  expect(response.status()).toBe(202)
}

/** A `pull_request` delivery for the issue's branch: number 7 on the fixture's head commit unless given. */
async function deliverPullRequest(
  request: APIRequestContext,
  id: string,
  action: 'opened' | 'ready_for_review',
  head: { number: number; sha: string } = { number: 7, sha: '6dcb09b5b57875f334f61aebed695e2e4193db5e' },
) {
  const payload = fixture(`pull_request_${action}`) as {
    repository: { name: string }
    pull_request: { number: number; head: { ref: string; sha: string } }
  }
  payload.pull_request.number = head.number
  payload.pull_request.head.ref = `feature/${id.toLowerCase()}-live`
  payload.pull_request.head.sha = head.sha
  await deliver(request, 'pull_request', payload)
}

/** A completed check run named `e2e` on the head commit of the issue's pull request. */
async function deliverCheck(
  request: APIRequestContext,
  id: string,
  head: { number: number; sha: string },
  conclusion: 'success' | 'failure',
) {
  const payload = fixture('check_run_completed') as {
    repository: { name: string }
    check_run: {
      name: string
      head_sha: string
      conclusion: string
      pull_requests: { number: number; head: { ref: string; sha: string } }[]
    }
  }
  Object.assign(payload.check_run, { name: 'e2e', head_sha: head.sha, conclusion })
  const linked = payload.check_run.pull_requests[0]
  if (linked) Object.assign(linked, { number: head.number, head: { ...linked.head, ref: `feature/${id.toLowerCase()}-live`, sha: head.sha } })
  await deliver(request, 'check_run', payload)
}

async function signIn(page: Page) {
  await page.goto('/')
  await expect(page).toHaveURL(/\/sign-in$/)
  await page.getByLabel('Token').fill(personToken())
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page).toHaveURL(/\/$/)
}

const row = (page: Page, id: string) => page.locator(`[data-issue="${id}"]`)

// A click selects a row and a click on the selected row opens its sheet; Enter opens the issue page.
async function openSheet(page: Page, id: string) {
  // A sheet still closing would take the clicks.
  await expect(page.getByRole('dialog')).toHaveCount(0)
  if ((await row(page, id).getAttribute('aria-selected')) !== 'true') await row(page, id).click()
  await expect(row(page, id)).toHaveAttribute('aria-selected', 'true')
  await row(page, id).click()
  return page.getByRole('dialog')
}

// First in the file: its runner claims the dispatch its own run's end queues, before another test's runner could.
test('a run ending, the next claim and a pull request turning ready show on the open list', async ({
  page,
  request,
}) => {
  const { id, runner, claim } = await claimedInProgress(request, 'Updated live')
  await signIn(page)
  const run = row(page, id).locator('[data-run]')
  const pr = row(page, id).locator('[data-pr]')
  await expect(run).toHaveAttribute('data-run', 'running')
  await page.evaluate(() => {
    sessionStorage.setItem('loaded', String(performance.timeOrigin))
  })
  const live = { timeout: 2000 }

  await ok(
    await request.post(`/api/runs/${claim.run_id}/finish`, {
      headers: bearer(runner),
      data: { attempt: claim.attempt, outcome: 'failed', reason: 'e2e', duration_ms: 1 },
    }),
  )
  await expect(run).toHaveText('Queued', live)
  await ok(await request.post('/api/runs/claim', { headers: bearer(runner), data: {} }))
  await expect(run).toHaveAttribute('data-run', 'running', live)

  await deliverPullRequest(request, id, 'opened')
  await expect(pr).toHaveText('#7 draft', live)
  await deliverPullRequest(request, id, 'ready_for_review')
  await expect(pr).toHaveText('#7 open', live)
  await expect(row(page, id).locator('[data-gates]')).toHaveAttribute('data-gates', '3/3', live)
  // The same document all along: a reload would have a new time origin.
  expect(await page.evaluate(() => sessionStorage.getItem('loaded') === String(performance.timeOrigin))).toBe(true)
})

test('create, edit, roll back a refused edit, and show a refused transition as pending then refused', async ({
  page,
  request,
}) => {
  const inProgress = await issueInProgress(request, 'Claimed by a runner')
  await signIn(page)
  await expect(row(page, inProgress)).toBeVisible()

  await page.getByRole('button', { name: 'New issue' }).click()
  await page.getByRole('dialog').getByLabel('Title').fill('Written in the browser')
  await page.getByRole('dialog').getByLabel('Body').fill('From the e2e test')
  await page.getByRole('button', { name: 'Create issue' }).click()
  const created = page.locator('[data-group="backlog"] [data-issue]', { hasText: 'Written in the browser' })
  await expect(created).toBeVisible()
  const id = await created.getAttribute('data-issue')
  if (!id) throw new Error('the created row has no id')

  let sheet = await openSheet(page, id)
  await sheet.getByLabel('Title').fill('Renamed in the browser')
  await sheet.getByLabel('Title').press('Enter')
  await expect(row(page, id)).toContainText('Renamed in the browser')
  await page.reload()
  await expect(row(page, id)).toContainText('Renamed in the browser')

  await page.route(`**/api/issues/${id}`, (route) =>
    route.request().method() === 'PATCH'
      ? route.fulfill({ status: 409, json: { code: 'conflict', message: 'the issue changed meanwhile' } })
      : route.continue(),
  )
  sheet = await openSheet(page, id)
  await sheet.getByLabel('Title').fill('Refused title')
  await sheet.getByLabel('Title').press('Enter')
  await expect(page.getByText(`${id} was not changed. Refused (conflict): the issue changed meanwhile`)).toBeVisible()
  await expect(sheet.getByLabel('Title')).toHaveValue('Renamed in the browser')
  await page.keyboard.press('Escape')
  await expect(row(page, id)).toContainText('Renamed in the browser')
  await expect(row(page, id)).not.toContainText('Refused title')

  let release: () => void = () => undefined
  const held = new Promise<void>((resolve) => (release = resolve))
  await page.route(`**/api/issues/${inProgress}/transitions`, async (route) => {
    await held
    await route.continue()
  })
  await row(page, inProgress).click()
  await page.keyboard.press('s')
  await page.getByRole('menuitemradio', { name: 'Agent Review' }).click()
  await expect(row(page, inProgress).locator('[data-transition="pending"]')).toHaveText('Moving to Agent Review…')
  release()
  await expect(row(page, inProgress).locator('[data-transition="refused"]')).toHaveText(
    'Agent Review: Refused (role_not_allowed): only an agent makes in_progress → agent_review, not a person',
  )
  await expect(page.locator(`[data-group="in_progress"] [data-issue="${inProgress}"]`)).toBeVisible()
  const stored = await ok<{ status: string }>(
    await request.get(`/api/issues/${inProgress}`, { headers: bearer(personToken()) }),
  )
  expect(stored.status).toBe('in_progress')
})

test('set a parent, project and milestone in the sheet, and roll back a parent cycle', async ({ page, request }) => {
  const person = bearer(personToken())
  const create = async (title: string) =>
    (
      await ok<{ row: { issue: { id: string } } }>(
        await request.post('/api/issues', { headers: person, data: { title, body: '' } }),
      )
    ).row.issue.id
  const top = await create('Top of the tree')
  const child = await create('Under the top')
  const name = `Project of ${top}`
  const project = await ok<{ id: string }>(await request.post('/api/projects', { headers: person, data: { name } }))
  await ok(
    await request.post(`/api/projects/${project.id}/milestones`, {
      headers: person,
      data: { name: 'Gate 0', pass_when: 'the tree shows in the sheet' },
    }),
  )
  await signIn(page)

  let sheet = await openSheet(page, child)
  await sheet.getByLabel('Parent').fill(top)
  await sheet.getByLabel('Parent').press('Enter')
  await expect(sheet.getByText('Top of the tree')).toBeVisible()
  // The project's edit is held, so both selects show optimistic edits. The milestone's edit must reach the server
  // after the project's: before it, the server would check the milestone against no project.
  let release: () => void = () => undefined
  const held = new Promise<void>((resolve) => (release = resolve))
  let patches = 0
  await page.route(`**/api/issues/${child}`, async (route) => {
    if (route.request().method() === 'PATCH' && patches++ === 0) await held
    await route.continue()
  })
  await sheet.getByLabel('Project').click()
  await page.getByRole('option', { name }).click()
  await expect(sheet.getByLabel('Project')).toHaveText(name)
  await sheet.getByLabel('Milestone').click()
  await page.getByRole('option', { name: 'Gate 0' }).click()
  await expect(sheet.getByLabel('Milestone')).toHaveText('Gate 0')
  await expect(sheet.getByText('PASS when the tree shows in the sheet')).toBeVisible()
  release()
  await expect
    .poll(async () => {
      const issue = await ok<{ milestone: { name: string } | null }>(
        await request.get(`/api/issues/${child}`, { headers: person }),
      )
      return issue.milestone?.name
    })
    .toBe('Gate 0')
  await expect(page.getByText(`${child} was not changed.`)).toHaveCount(0)
  await page.unroute(`**/api/issues/${child}`)
  await page.reload()
  sheet = await openSheet(page, child)
  await expect(sheet.getByLabel('Parent')).toHaveValue(top)
  await expect(sheet.getByLabel('Project')).toHaveText(name)
  await expect(sheet.getByLabel('Milestone')).toHaveText('Gate 0')
  await page.keyboard.press('Escape')

  sheet = await openSheet(page, top)
  await sheet.getByLabel('Parent').fill(child)
  await sheet.getByLabel('Parent').press('Enter')
  await expect(
    page.getByText(
      `${top} was not changed. Refused (invalid): ${child} is under ${top} (${top} → ${child}), so it cannot be ${top}'s parent`,
    ),
  ).toBeVisible()
  await expect(sheet.getByLabel('Parent')).toHaveValue('')
  const stored = await ok<{ parent: string | null }>(await request.get(`/api/issues/${top}`, { headers: person }))
  expect(stored.parent).toBeNull()
})

// The issue page. These come last: their runs and dispatches must not be what an earlier test's runner claims.

/** A 1×1 PNG. */
const PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==',
  'base64',
)

test('the Human Review move shows blocked by ci_green, then a standalone agent is locked out there', async ({
  browser,
  request,
}) => {
  const person = bearer(personToken())
  const { id, runner, claim } = await claimedInProgress(request, 'Blocked by a check')
  const head = { number: 300 + Number(id.split('-')[1]), sha: randomUUID().replaceAll('-', '').padEnd(40, '0') }
  await deliverPullRequest(request, id, 'opened', head)
  await deliverPullRequest(request, id, 'ready_for_review', head)
  const agent = await ok<{ token: string }>(await request.post('/api/actors', { headers: person, data: { kind: 'codex' } }))
  await ok(await request.post(`/api/issues/${id}/transitions`, { headers: bearer(agent.token), data: { to: 'agent_review' } }))
  // Ended after the move, so the end queues no run in In Progress.
  await ok(
    await request.post(`/api/runs/${claim.run_id}/finish`, {
      headers: bearer(runner),
      data: { attempt: claim.attempt, outcome: 'succeeded', duration_ms: 1 },
    }),
  )
  await deliverCheck(request, id, head, 'failure')

  const context = await browser.newContext({
    baseURL: process.env.E2E_BASE_URL,
    extraHTTPHeaders: bearer(agent.token),
  })
  const page = await context.newPage()
  await page.goto(`/issues/${id}`)
  await expect(page.locator('[data-viewer]').first()).toHaveAttribute('data-viewer', 'agent')
  const move = page.locator('[data-transition="human_review"]')
  await expect(move).toHaveAttribute('data-verdict', 'blocked')
  await expect(move.locator('[data-blocked-gate]')).toHaveText('ci_green')
  await expect(move.locator('[data-blocked-names]')).toHaveText('`e2e` (failure)')
  await page.locator('[data-gate="ci_green"] [data-evidence]').click()
  await expect(page.locator('#checks')).toBeInViewport()
  await expect(page.locator('#checks [data-check="e2e"]')).toHaveAttribute('data-check-state', 'failing')

  await deliverCheck(request, id, head, 'success')
  const upload = await request.post(`/api/issues/${id}/proof`, {
    headers: bearer(agent.token),
    multipart: { title: 'Screenshot', file: { name: 'shot.png', mimeType: 'image/png', buffer: PNG } },
  })
  expect(upload.status()).toBe(201)
  await ok(await request.post(`/api/issues/${id}/transitions`, { headers: bearer(agent.token), data: { to: 'human_review' } }))
  await page.reload()
  await expect(page.locator('[data-notice="agent-locked"]')).toContainText('Agents cannot act on this issue.')
  await expect(page.locator('[data-transition="merging"]')).toHaveAttribute('aria-disabled', 'true')
  await expect(page.getByLabel('New comment')).toBeDisabled()
  await expect(page.locator('[data-composer-note]')).toContainText('Human Review is locked to people.')
  await context.close()
})

test('a screenshot uploaded from the issue page is the person\'s proof and shows its thumbnail', async ({
  page,
  request,
}) => {
  const person = bearer(personToken())
  const { row: created } = await ok<{ row: { issue: { id: string } } }>(
    await request.post('/api/issues', { headers: person, data: { title: 'Needs a screenshot', body: '' } }),
  )
  const id = created.issue.id
  await signIn(page)
  await page.goto(`/issues/${id}`)
  await expect(page.locator('[data-proof-empty]')).toBeVisible()
  await page.locator('[data-proof-input]').setInputFiles({ name: 'board.png', mimeType: 'image/png', buffer: PNG })
  const thumb = page.locator('[data-proof="image"] img')
  await expect(thumb).toHaveAttribute('alt', 'board.png')
  await expect.poll(() => thumb.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBe(1)
  const stored = await ok<{ title: string; kind: string; actor: { name: string; role: string } }[]>(
    await request.get(`/api/issues/${id}/proof`, { headers: person }),
  )
  expect(stored.map(({ title, kind, actor }) => [title, kind, actor.name, actor.role])).toEqual([
    ['board.png', 'image', 'Verify Person', 'person'],
  ])
})

test('Enter opens the issue page, and a comment from its composer shows in the feed and in pl issue show', async ({
  page,
  request,
}) => {
  const person = bearer(personToken())
  const { row: created } = await ok<{ row: { issue: { id: string } } }>(
    await request.post('/api/issues', { headers: person, data: { title: 'Talk about it', body: '' } }),
  )
  const id = created.issue.id
  await signIn(page)
  await row(page, id).click()
  await page.keyboard.press('Enter')
  await expect(page).toHaveURL(new RegExp(`/issues/${id}$`))
  const text = `Commented from the page ${randomUUID()}`
  await page.keyboard.press('c')
  await expect(page.getByLabel('New comment')).toBeFocused()
  await page.keyboard.type(text)
  await page.getByRole('button', { name: 'Comment', exact: true }).click()
  await expect(page.locator('[data-comment]', { hasText: text })).toBeVisible({ timeout: 2000 })
  await expect(page.getByLabel('New comment')).toHaveValue('')
  await page.locator('[data-feed="comments"]').click()
  await expect(page.locator('[data-kind]')).toHaveCount(1)

  const pl = new URL('../../target/debug/pl', import.meta.url).pathname
  const shown = execFileSync(pl, ['issue', 'show', id], { env: { ...process.env, PUNCHLIST_CONFIG: process.env.E2E_CONFIG } })
  expect(shown.toString()).toContain(text)
  await page.keyboard.press('Escape')
  await expect(page).toHaveURL(/\/$/)
})
