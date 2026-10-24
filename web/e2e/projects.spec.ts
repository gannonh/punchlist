import { expect, test } from '@playwright/test'
import { bearer, ok, personToken, signIn } from './support'

test('the projects screen shows a milestone with its PASS when and progress, and opens its issues', async ({
  page,
  request,
}) => {
  const person = bearer(personToken())
  // Names other spec files do not use, since they share the workspace.
  const name = `Projects screen ${String(Date.now())}`
  const project = await ok<{ id: string }>(await request.post('/api/projects', { headers: person, data: { name } }))
  const milestone = await ok<{ id: string }>(
    await request.post(`/api/projects/${project.id}/milestones`, {
      headers: person,
      data: { name: 'Gate 1', pass_when: 'Line one\n<b>Line two</b>' },
    }),
  )
  const ids: string[] = []
  for (const title of ['First in the gate', 'Second in the gate']) {
    const { row } = await ok<{ row: { issue: { id: string } } }>(
      await request.post('/api/issues', { headers: person, data: { title, body: '' } }),
    )
    await ok(
      await request.patch(`/api/issues/${row.issue.id}`, {
        headers: person,
        data: { project: project.id, milestone: milestone.id },
      }),
    )
    ids.push(row.issue.id)
  }

  await signIn(page)
  await page.keyboard.press('g')
  await page.keyboard.press('m')
  await expect(page).toHaveURL(/\/w\/PL\/projects$/)
  await expect(page.locator('header [aria-current=page]')).toHaveText('Projects')

  const row = page.locator(`[data-milestone="${milestone.id}"]`)
  await expect(row.locator('[data-done-count]')).toHaveText('0 of 2 done')
  // HTML in the PASS when text is characters, with its line break kept.
  expect(await row.locator('[data-pass-when]').innerText()).toContain('Line one\n<b>Line two</b>')
  await expect(row.getByRole('img', { name: 'Backlog: 2' })).toBeVisible()

  await row.getByRole('button', { name: 'Gate 1' }).click()
  for (const id of ids) await expect(row.locator(`[data-issue-link="${id}"]`)).toHaveAttribute('href', `/issues/${id}`)

  await row.getByRole('link', { name: /Show in list/ }).click()
  await expect(page).toHaveURL(new RegExp(`/w/PL\\?milestone=${milestone.id}$`))
})
