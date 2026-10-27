import { expect, test, type APIRequestContext } from '@playwright/test'
import { personToken, signIn } from './support'

type CliLogin = { code: string; url: string; device_secret: string }

// What `pl auth login` sends: no token, its host in the user agent.
async function startLogin(request: APIRequestContext): Promise<CliLogin> {
  const response = await request.post('/api/cli-logins', { headers: { 'user-agent': 'pl/0.1.0 (e2e-box)' } })
  expect(response.status()).toBe(201)
  return (await response.json()) as CliLogin
}

const poll = async (request: APIRequestContext, secret: string) =>
  (await (await request.post('/api/cli-logins/token', { data: { device_secret: secret } })).json()) as {
    state: string
    token?: string
  }

const path = (login: CliLogin) => new URL(login.url).pathname + new URL(login.url).search

test('a person approves pl on the approval page, and pl gets a session that works', async ({ page, request }) => {
  const login = await startLogin(request)
  await signIn(page)
  await page.goto(path(login))
  await expect(page.getByRole('heading', { name: 'Sign in pl on e2e-box as Verify Person?' })).toBeVisible()
  await expect(page.locator('[data-code]')).toHaveText(login.code)
  await page.getByRole('button', { name: 'Approve' }).click()
  await expect(page.locator('[data-result="approved"]')).toHaveText(
    'Approved. pl on e2e-box is signed in; you can close this tab.',
  )
  const answer = await poll(request, login.device_secret)
  expect(answer.state).toBe('approved')
  const issues = await request.get('/api/issues', { headers: { Authorization: `Bearer ${String(answer.token)}` } })
  expect(issues.status()).toBe(200)
  // Approved once: a second approval is refused.
  const again = await page.request.post(`/api/cli-logins/${login.code}/approve`, {
    headers: { origin: new URL(page.url()).origin },
  })
  expect(again.status()).toBe(409)
})

test('a person denies pl, and pl is told so', async ({ page, request }) => {
  const login = await startLogin(request)
  await signIn(page)
  await page.goto(path(login))
  await page.getByRole('button', { name: 'Deny' }).click()
  await expect(page.locator('[data-result="denied"]')).toHaveText('Denied. pl was not signed in.')
  expect(await poll(request, login.device_secret)).toEqual({ state: 'denied' })
})

test('signed out, the approval page goes through the sign-in page and back', async ({ page, request }) => {
  const login = await startLogin(request)
  await page.goto(path(login))
  await expect(page).toHaveURL(/\/sign-in$/)
  await page.getByLabel('Token').fill(personToken())
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page).toHaveURL(new RegExp(`/cli-login\\?code=${login.code}$`))
  await expect(page.getByRole('button', { name: 'Approve' })).toBeVisible()
})
