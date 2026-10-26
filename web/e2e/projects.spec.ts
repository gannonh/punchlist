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

  await row.getByRole('button', { name: 'Gate 1', exact: true }).click()
  for (const id of ids) await expect(row.locator(`[data-issue-link="${id}"]`)).toHaveAttribute('href', `/issues/${id}`)

  await row.getByRole('link', { name: /Show in list/ }).click()
  await expect(page).toHaveURL(new RegExp(`/w/PL\\?milestone=${milestone.id}$`))
})

test('the projects screen creates and edits projects and milestones, and shows a refusal in the form', async ({
  page,
  request,
}) => {
  const person = bearer(personToken())
  const stamp = String(Date.now())
  const site = `Site ${stamp}`
  await signIn(page)
  await page.goto('/w/PL/projects')

  await page.getByRole('button', { name: '+ New project' }).click()
  const dialog = page.getByRole('dialog')
  await expect(dialog.getByRole('heading', { name: 'New project' })).toBeVisible()
  await dialog.getByLabel('Name').fill(site)
  await dialog.getByRole('button', { name: 'Create project' }).click()
  await expect(dialog).toHaveCount(0)
  const section = page.locator('section[data-project]').filter({ has: page.getByRole('heading', { name: site }) })
  await expect(section).toHaveCount(1)

  // A refused save keeps the form open with the server's message; Esc closes it.
  await page.getByRole('button', { name: '+ New project' }).click()
  await dialog.getByLabel('Name').fill(site)
  await dialog.getByLabel('Name').press('Enter')
  await expect(dialog.locator('[data-form-error]')).toHaveText(`a project named ${site} already exists`)
  // Off the fields too: Radix closes the dialog before the screen's own Esc handler runs.
  await dialog.getByRole('heading', { name: 'New project' }).click()
  await page.keyboard.press('Escape')
  await expect(dialog).toHaveCount(0)
  await expect(page).toHaveURL(/\/w\/PL\/projects$/)

  await section.getByRole('button', { name: /New milestone/ }).click()
  await dialog.getByLabel('Name').fill('Launch')
  await dialog.getByLabel('PASS when').fill('<img src=x onerror=alert(1)>')
  await dialog.getByRole('button', { name: 'Create milestone' }).click()
  await expect(dialog).toHaveCount(0)
  const milestone = section.locator('[data-milestone]')
  await expect(milestone.locator('[data-name]')).toHaveText('Launch')
  await expect(milestone.locator('[data-done-count]')).toHaveText('0 of 0 done')
  await expect(milestone.locator('[data-pass-when]')).toContainText('<img src=x onerror=alert(1)>')
  await expect(milestone.locator('img')).toHaveCount(0)

  await milestone.getByRole('button', { name: /^Edit milestone/ }).click()
  await expect(dialog.getByLabel('Name')).toHaveValue('Launch')
  await dialog.getByLabel('Name').fill('Go live')
  await dialog.getByLabel('PASS when').fill('Public and documented')
  await dialog.getByRole('button', { name: 'Save' }).click()
  await expect(dialog).toHaveCount(0)
  await expect(milestone.locator('[data-name]')).toHaveText('Go live')
  await expect(milestone.locator('[data-pass-when]')).toContainText('Public and documented')

  await section.getByRole('button', { name: /^Rename project/ }).click()
  await dialog.getByLabel('Name').fill('')
  await dialog.getByRole('button', { name: 'Save' }).click()
  await expect(dialog.locator('[data-form-error]')).toHaveText('a project needs a name')
  await dialog.getByLabel('Name').fill(`${site} renamed`)
  await dialog.getByRole('button', { name: 'Save' }).click()
  await expect(dialog).toHaveCount(0)
  await expect(page.getByRole('heading', { name: `${site} renamed` })).toBeVisible()

  const { projects } = await ok<{ projects: { name: string; milestones: { name: string; pass_when: string }[] }[] }>(
    await request.get('/api/projects', { headers: person }),
  )
  expect(projects.find((p) => p.name === `${site} renamed`)?.milestones).toEqual([
    expect.objectContaining({ name: 'Go live', pass_when: 'Public and documented' }),
  ])
})

test('a rename shows on an open list and projects screen without a reload', async ({ page, request }) => {
  const person = bearer(personToken())
  const name = `Live rename ${String(Date.now())}`
  const project = await ok<{ id: string }>(await request.post('/api/projects', { headers: person, data: { name } }))
  const { row } = await ok<{ row: { issue: { id: string } } }>(
    await request.post('/api/issues', { headers: person, data: { title: 'Placed for a rename', body: '' } }),
  )
  await ok(await request.patch(`/api/issues/${row.issue.id}`, { headers: person, data: { project: project.id } }))
  await signIn(page)
  const projectsPage = await page.context().newPage()
  await projectsPage.goto('/w/PL/projects')
  await expect(projectsPage.getByRole('heading', { name })).toBeVisible()

  const listed = page.locator(`[data-issue="${row.issue.id}"]`)
  await listed.click()
  await expect(listed).toHaveAttribute('aria-selected', 'true')
  await listed.click()
  const sheet = page.getByRole('dialog')
  await expect(sheet.getByLabel('Project')).toHaveText(name)

  await ok(await request.patch(`/api/projects/${project.id}`, { headers: person, data: { name: `${name} renamed` } }))
  await expect(sheet.getByLabel('Project')).toHaveText(`${name} renamed`, { timeout: 2000 })
  await expect(projectsPage.getByRole('heading', { name: `${name} renamed` })).toBeVisible({ timeout: 2000 })
})
