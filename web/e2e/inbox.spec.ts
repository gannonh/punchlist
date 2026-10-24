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

  await item.focus()
  await page.keyboard.press('Enter')
  await expect(page).toHaveURL(new RegExp(`/issues/${id}$`))
  expect((await inbox(request)).items.find((item) => item.issue.id === id)?.unread).toBe(false)

  await page.keyboard.press('Escape')
  await expect(page).toHaveURL(/\/w\/PL$/)
  await headerMatches(page, request)
})
