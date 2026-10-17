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

/** An issue in In Progress without a model: moved to Start by the person, then claimed by a runner through the API. */
async function issueInProgress(request: APIRequestContext, title: string): Promise<string> {
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
  const claim = await ok<{ issue: { id: string; status: string } }>(
    await request.post('/api/runs/claim', { headers: bearer(runner.token), data: {} }),
  )
  expect(claim.issue).toMatchObject({ id, status: 'in_progress' })
  return id
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
  // Held, so what the select shows is the optimistic edit.
  let release: () => void = () => undefined
  const held = new Promise<void>((resolve) => (release = resolve))
  await page.route(`**/api/issues/${child}`, async (route) => {
    if (route.request().method() === 'PATCH') await held
    await route.continue()
  })
  await sheet.getByLabel('Project').click()
  await page.getByRole('option', { name }).click()
  await expect(sheet.getByLabel('Project')).toHaveText(name)
  release()
  await sheet.getByLabel('Milestone').click()
  await page.getByRole('option', { name: 'Gate 0' }).click()
  await expect(sheet.getByText('PASS when the tree shows in the sheet')).toBeVisible()
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
