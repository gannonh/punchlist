import { randomUUID } from 'node:crypto'
import { expect, test, type Page } from '@playwright/test'
import { bearer, ok, personToken, signIn } from './support'

test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true })

const strip = (page: Page, action: string) => page.locator(`[data-strip] [data-action="${action}"]`)
const typing = (page: Page) =>
  page.evaluate(() => {
    const active = document.activeElement
    return active instanceof HTMLElement && (active.isContentEditable || ['INPUT', 'TEXTAREA'].includes(active.tagName))
  })

test('on a phone the strip walks, opens and moves an issue by tap, and c focuses the composer', async ({
  page,
  request,
}) => {
  const person = bearer(personToken())
  const label = `strip-${randomUUID().slice(0, 8)}`
  for (const title of ['Strip one', 'Strip two', 'Strip three']) {
    const { row } = await ok<{ row: { issue: { id: string } } }>(
      await request.post('/api/issues', { headers: person, data: { title, body: '' } }),
    )
    await ok(await request.patch(`/api/issues/${row.issue.id}`, { headers: person, data: { labels: [label] } }))
  }
  await signIn(page)
  await page.goto(`/w/PL?label=${label}`)
  await expect(page.locator('[data-issue]')).toHaveCount(3)
  const ids = await page.locator('[data-issue]').evaluateAll((rows) => rows.map((r) => r.getAttribute('data-issue') ?? ''))
  expect(ids).toHaveLength(3)
  const [first, second] = ids as [string, string, string]
  await expect(page.locator(`[data-issue="${first}"]`)).toHaveAttribute('data-focused', '')

  // Each button is a touch target, and the strip scrolls sideways rather than wrapping.
  const heights = await page.locator('[data-strip] button').evaluateAll((buttons) =>
    buttons.map((b) => b.getBoundingClientRect().height),
  )
  expect(Math.min(...heights)).toBeGreaterThanOrEqual(44)

  // Only the filter puts the keyboard up, and the next tap takes it down.
  await strip(page, 'filter').tap()
  expect(await typing(page)).toBe(true)
  await strip(page, 'next').tap()
  expect(await typing(page)).toBe(false)
  await expect(page.locator(`[data-issue="${second}"]`)).toHaveAttribute('data-focused', '')

  await strip(page, 'status').tap()
  await page.getByRole('menuitemradio', { name: 'Todo' }).tap()
  await expect
    .poll(async () => (await ok<{ status: string }>(await request.get(`/api/issues/${second}`, { headers: person }))).status)
    .toBe('todo')

  await expect(page.locator(`[data-issue="${second}"]`)).toHaveAttribute('data-focused', '')
  await strip(page, 'open').tap()
  await expect(page).toHaveURL(new RegExp(`/issues/${second}$`))

  await expect(page.getByLabel('New comment')).toBeEnabled()
  await strip(page, 'gates').tap()
  expect(await typing(page)).toBe(false)
  await strip(page, 'comment').tap()
  await expect(page.getByLabel('New comment')).toBeFocused()
  await strip(page, 'issues').tap()
  await expect(page).toHaveURL(/\/w\/PL$/)
})

test('with no focused row the row actions are disabled and say why', async ({ page }) => {
  await signIn(page)
  await page.goto(`/w/PL?label=none-${randomUUID().slice(0, 8)}`)
  await expect(page.locator('[data-empty]')).toBeVisible()
  for (const action of ['status', 'select', 'assignee', 'labels', 'project', 'milestone']) {
    await expect(strip(page, action)).toBeDisabled()
    await expect(strip(page, action)).toHaveAttribute('title', 'Focus an issue first')
  }
})
