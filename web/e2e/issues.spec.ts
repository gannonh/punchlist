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
  const claim = await ok<Claimed['claim'] & { issue: { id: string; status: string } }>(
    await request.post('/api/runs/claim', { headers: bearer(runner.token), data: {} }),
  )
  expect(claim.issue).toMatchObject({ id, status: 'in_progress' })
  return { id, runner: runner.token, claim }
}

const issueInProgress = async (request: APIRequestContext, title: string) =>
  (await claimedInProgress(request, title)).id

/** A `pull_request` delivery for the issue's branch on the run's repository, signed as GitHub signs one. */
async function deliverPullRequest(request: APIRequestContext, id: string, action: 'opened' | 'ready_for_review') {
  const secret = process.env.E2E_WEBHOOK_SECRET_FILE
  if (!secret) throw new Error('E2E_WEBHOOK_SECRET_FILE is not set; run `pnpm e2e`')
  const fixture = new URL(`../../crates/punchlist-server/tests/fixtures/github/pull_request_${action}.json`, import.meta.url)
  const payload = JSON.parse(readFileSync(fixture, 'utf8')) as {
    repository: { name: string }
    pull_request: { number: number; head: { ref: string } }
  }
  payload.repository.name = 'punchlist-sandbox'
  payload.pull_request.number = 7
  payload.pull_request.head.ref = `feature/${id.toLowerCase()}-live`
  const body = JSON.stringify(payload)
  const signature = createHmac('sha256', readFileSync(secret, 'utf8')).update(body).digest('hex')
  const response = await request.post('/api/github/webhook', {
    headers: {
      'content-type': 'application/json',
      'x-github-event': 'pull_request',
      'x-github-delivery': randomUUID(),
      'x-hub-signature-256': `sha256=${signature}`,
    },
    data: body,
  })
  expect(response.status()).toBe(202)
}

async function signIn(page: Page) {
  await page.goto('/')
  await expect(page).toHaveURL(/\/sign-in$/)
  await page.getByLabel('Token').fill(personToken())
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page).toHaveURL(/\/$/)
}

const row = (page: Page, id: string) => page.locator(`[data-issue="${id}"]`)

async function openSheet(page: Page, id: string) {
  await row(page, id).click()
  await expect(row(page, id)).toHaveAttribute('aria-selected', 'true')
  await page.keyboard.press('Enter')
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
