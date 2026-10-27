import { expect, test } from '@playwright/test'
import { bearer, headerMatches, inbox, ok, personToken, signIn } from './support'

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

  // Esc on a menu item closes the workspace menu and the page stays.
  await page.locator('[data-workspace-menu]').focus()
  await page.keyboard.press('Enter')
  await expect(page.getByRole('menuitemradio').first()).toBeFocused()
  await page.keyboard.press('Escape')
  await expect(page.getByRole('menu')).toHaveCount(0)
  await expect(page).toHaveURL(/\/w\/PL\/inbox$/)
  await expect(page.locator('[data-workspace-menu]')).toBeFocused()
  // The account menu's Sessions dialog too, with focus on no control: Radix closes it before the page's Esc runs.
  await page.locator('[data-account-menu]').click()
  await page.locator('[data-sessions-item]').click()
  await page.locator('[data-sessions]').getByRole('heading', { name: 'Sessions' }).click()
  await page.keyboard.press('Escape')
  await expect(page.locator('[data-sessions]')).toHaveCount(0)
  await expect(page).toHaveURL(/\/w\/PL\/inbox$/)

  await item.focus()
  await page.keyboard.press('Enter')
  await expect(page).toHaveURL(new RegExp(`/issues/${id}$`))
  expect((await inbox(request)).items.find((item) => item.issue.id === id)?.unread).toBe(false)

  await page.keyboard.press('Escape')
  await expect(page).toHaveURL(/\/w\/PL$/)
  await headerMatches(page, request)
})
