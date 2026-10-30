import { randomUUID } from 'node:crypto'
import { expect, test } from '@playwright/test'
import { bearer, deliver, deliverPullRequest, fixture, ok, personToken, signIn } from './support'

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

  // Esc on a menu item closes the workspace menu and the page stays.
  await page.locator('[data-workspace-menu]').focus()
  await page.keyboard.press('Enter')
  await expect(page.getByRole('menuitemradio').first()).toBeFocused()
  await page.keyboard.press('Escape')
  await expect(page.getByRole('menu')).toHaveCount(0)
  await expect(page).toHaveURL(/\/w\/PL\/pull-requests$/)
  await expect(page.locator('[data-workspace-menu]')).toBeFocused()
  // The account menu's Sessions dialog too, with focus on no control: Radix closes it before the page's Esc runs.
  await page.locator('[data-account-menu]').click()
  await page.locator('[data-sessions-item]').click()
  await page.locator('[data-sessions]').getByRole('heading', { name: 'Sessions' }).click()
  await page.keyboard.press('Escape')
  await expect(page.locator('[data-sessions]')).toHaveCount(0)
  await expect(page).toHaveURL(/\/w\/PL\/pull-requests$/)

  await pr.focus()
  await page.keyboard.press('Enter')
  await expect(page).toHaveURL(new RegExp(`/issues/${id}$`))
})

test('the Pull requests page reads again every 30 s while visible, so a pull request that names no issue shows', async ({
  page,
  request,
}) => {
  // No event reaches the page: the stream answers an empty body, and EventSource keeps reconnecting to it.
  await page.route('**/api/workspaces/*/events?*', (route) =>
    route.fulfill({ status: 200, contentType: 'text/event-stream', body: '' }),
  )
  let reads = 0
  page.on('request', (r) => {
    if (/\/api\/workspaces\/[^/]+\/pull-requests$/.test(new URL(r.url()).pathname)) reads += 1
  })
  // Keep initial fetches, React's notifications and the refetch interval on one
  // controlled timeline. A running clock plus fastForward can jump before the
  // observer has finished installing/resetting its interval (PL-186).
  await page.clock.install({ time: new Date('2026-01-01T00:00:00Z') })
  await page.clock.pauseAt(new Date('2026-01-01T00:00:01Z'))
  await signIn(page)
  // Counted requests can still be in flight: deliver only after the initial snapshot
  // has arrived, or it can already contain the PR whose timed refresh we are testing.
  const initialRead = page.waitForResponse((response) =>
    /\/api\/workspaces\/[^/]+\/pull-requests$/.test(new URL(response.url()).pathname),
  )
  await page.keyboard.press('g')
  await page.keyboard.press('p')
  await expect(page).toHaveURL(/\/pull-requests$/)
  const initialResponse = await initialRead
  await initialResponse.finished()
  await page.clock.runFor(1_000)
  await expect(page.getByRole('heading', { level: 1 })).toContainText(/\d+ open/)
  await expect.poll(() => reads).toBe(1)

  const number = 9000 + Math.floor(Math.random() * 900_000)
  const payload = fixture('pull_request_opened') as {
    repository: { name: string }
    pull_request: { number: number; title: string; html_url: string; head: { ref: string; sha: string } }
  }
  payload.pull_request.number = number
  payload.pull_request.title = 'Names no issue'
  payload.pull_request.html_url = `https://github.com/gannonh/punchlist-sandbox/pull/${String(number)}`
  payload.pull_request.head.ref = `chore/no-issue-${String(number)}`
  payload.pull_request.head.sha = randomUUID().replaceAll('-', '').padEnd(40, '0')
  await deliver(request, 'pull_request', payload)
  // A webhook's 202 only queues its job. Establish the server's new snapshot
  // before advancing the browser's timer, without refetching in the browser.
  await expect
    .poll(async () => {
      const pulls = await ok<{ number: number }[]>(
        await request.get(initialResponse.url(), { headers: bearer(personToken()) }),
      )
      return pulls.some((pull) => pull.number === number)
    })
    .toBe(true)
  const pr = page.locator('[data-section="names-no-issue"]').locator(`[data-pr-row="${String(number)}"]`)
  // The first timed read is due 30 s after the initial snapshot, settled above.
  await page.clock.runFor(25_000)
  expect(reads).toBe(1)
  await expect(pr).toHaveCount(0)
  await page.clock.runFor(5_000)
  await expect
    .poll(async () => {
      // Query notifications are timers too; allow the completed fetch to render
      // while keeping the next 30 s poll out of reach.
      await page.clock.runFor(10)
      return pr.isVisible()
    })
    .toBe(true)
  expect(reads).toBe(2)

  // A hidden tab sends no timed read.
  await page.evaluate(() => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' })
    document.dispatchEvent(new Event('visibilitychange'))
  })
  await page.clock.runFor(70_000)
  expect(reads).toBe(2)

  // Visible again, the next tick reads.
  await page.evaluate(() => {
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' })
    document.dispatchEvent(new Event('visibilitychange'))
  })
  await page.clock.runFor(30_000)
  await expect.poll(() => reads).toBe(3)
})
