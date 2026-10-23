import { randomUUID } from 'node:crypto'
import { expect, test } from '@playwright/test'
import { bearer, deliverPullRequest, ok, personToken, signIn } from './support'

test('g then p opens the Pull requests page, which moves a pull request between sections as GitHub reports it', async ({
  page,
  request,
}) => {
  const { row } = await ok<{ row: { issue: { id: string } } }>(
    await request.post('/api/issues', { headers: bearer(personToken()), data: { title: 'Has a pull request', body: '' } }),
  )
  const id = row.issue.id
  const head = { number: 500 + Number(id.split('-')[1]), sha: randomUUID().replaceAll('-', '').padEnd(40, '0') }
  await signIn(page)
  await deliverPullRequest(request, id, 'opened', head)

  await page.keyboard.press('g')
  await page.keyboard.press('p')
  await expect(page).toHaveURL(/\/pull-requests$/)
  const pr = page.locator(`[data-pr-row="${String(head.number)}"]`)
  await expect(page.locator('[data-section="draft"]').locator(pr)).toBeVisible()
  await expect(pr).toContainText(`#${String(head.number)}`)
  await expect(pr).toContainText(id)
  await expect(pr).toContainText('Backlog')
  await expect(page.getByRole('link', { name: 'Pull requests' })).toHaveAttribute('aria-current', 'page')

  await deliverPullRequest(request, id, 'ready_for_review', head)
  await expect(page.locator('[data-section="in-review"]').locator(pr)).toBeVisible({ timeout: 2000 })
  await expect(page.locator('[data-section="draft"]').locator(pr)).toHaveCount(0)

  await pr.focus()
  await page.keyboard.press('Enter')
  await expect(page).toHaveURL(new RegExp(`/issues/${id}$`))
})
