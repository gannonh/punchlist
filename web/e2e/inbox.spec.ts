import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { bearer, ok, personToken, signIn } from './support'

interface Inbox {
  items: { issue: { id: string; status: string }; unread: boolean }[]
  unread: number
}

const inbox = async (request: APIRequestContext) =>
  ok<Inbox>(await request.get('/api/inbox', { headers: bearer(personToken()) }))

// Other spec files move issues meanwhile, so the header is checked against the inbox as the server has it then.
async function headerMatches(page: Page, request: APIRequestContext) {
  await expect
    .poll(
      async () => {
        const [shown, now] = [await page.locator('[data-inbox-count]').textContent(), await inbox(request)]
        return shown === String(now.unread)
      },
      { timeout: 2000 },
    )
    .toBe(true)
}

test('the header counts what waits on the person live, and opening an item reads it', async ({ page, request }) => {
  const { row } = await ok<{ row: { issue: { id: string } } }>(
    await request.post('/api/issues', { headers: bearer(personToken()), data: { title: 'Waits on a person', body: '' } }),
  )
  const id = row.issue.id
  await signIn(page)
  await headerMatches(page, request)
  expect((await inbox(request)).items.map((item) => item.issue.id)).not.toContain(id)

  // Into Todo, which waits on a person: the header counts it without a reload.
  await ok(
    await request.post(`/api/issues/${id}/transitions`, { headers: bearer(personToken()), data: { to: 'todo' } }),
  )
  expect((await inbox(request)).items.find((item) => item.issue.id === id)?.unread).toBe(true)
  await headerMatches(page, request)

  await page.keyboard.press('i')
  await expect(page).toHaveURL(/\/w\/PL\/inbox$/)
  const item = page.locator(`[data-item="${id}"]`)
  await expect(item).toHaveAttribute('data-unread', 'true')
  await expect(item).toContainText(id)
  await expect(item).toContainText('Waits on a person')
  await expect(item).toContainText('Todo')
  await expect(item.locator('[data-age]')).toHaveText(/^\d+[smhd] ago$/)
  await expect(page.locator('header [aria-current=page]')).toHaveText(/^Inbox/)

  await item.focus()
  await page.keyboard.press('Enter')
  await expect(page).toHaveURL(new RegExp(`/issues/${id}$`))
  expect((await inbox(request)).items.find((item) => item.issue.id === id)?.unread).toBe(false)

  await page.keyboard.press('Escape')
  await expect(page).toHaveURL(/\/w\/PL$/)
  await headerMatches(page, request)
})

test('Run again on a run item whose runs are used up queues a run, and the item leaves without opening the issue', async ({
  page,
  request,
}) => {
  const person = bearer(personToken())
  const { row } = await ok<{ row: { issue: { id: string } } }>(
    await request.post('/api/issues', { headers: person, data: { title: 'Ran out of runs', body: '' } }),
  )
  const id = row.issue.id
  for (const to of ['todo', 'start'])
    await ok(await request.post(`/api/issues/${id}/transitions`, { headers: person, data: { to } }))
  // The person registers the runner, so the runs' agents are theirs and the item is in their inbox.
  const runner = await ok<{ token: string }>(
    await request.post('/api/runners', { headers: person, data: { name: `e2e-${id}`, agents: ['claude-code'] } }),
  )
  // Three runs in In Progress that end without moving it. Claims go in issue order, so an earlier test's waiting
  // work can come first: this runner takes it too, and claims again.
  for (let runs = 0; runs < 3; runs++) {
    let claim: { run_id: string; attempt: number; issue: { id: string } }
    do {
      claim = await ok(await request.post('/api/runs/claim', { headers: bearer(runner.token), data: {} }))
    } while (claim.issue.id !== id)
    await ok(
      await request.post(`/api/runs/${claim.run_id}/finish`, {
        headers: bearer(runner.token),
        data: { attempt: claim.attempt, outcome: 'succeeded', duration_ms: 1 },
      }),
    )
  }
  expect((await inbox(request)).items.find((item) => item.issue.id === id)).toMatchObject({ kind: 'run' })

  await signIn(page)
  await page.goto('/w/PL/inbox')
  await expect(page.locator(`[data-item="${id}"]`)).toContainText('3 runs in In Progress ended without moving it')
  await expect(page.locator('[data-section="waiting"] [data-run-again]')).toHaveCount(0)
  const retries: string[] = []
  page.on('request', (sent) => {
    if (sent.method() === 'POST' && sent.url().endsWith('/retry')) retries.push(sent.url())
  })
  await page.getByRole('button', { name: `Run again ${id}` }).click()
  await expect(page.locator(`[data-item="${id}"]`)).toHaveCount(0, { timeout: 2000 })
  expect(new URL(page.url()).pathname).toBe('/w/PL/inbox')
  expect(retries).toEqual([expect.stringContaining(`/api/issues/${id}/retry`)])
  await headerMatches(page, request)
})
