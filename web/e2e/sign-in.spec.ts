import { expect, test } from '@playwright/test'
import { signIn } from './support'

// `pnpm e2e` runs the server with sign-in with GitHub on verify-punchlist's fake GitHub, whose authorize page has a
// button per user: `octocat` and `stranger`.

test('a person links GitHub from the account menu, then a fresh browser signs in with it', async ({ page, browser }) => {
  await signIn(page)
  await page.getByRole('button', { name: 'Account' }).click()
  await page.getByRole('menuitem', { name: 'Link GitHub' }).click()
  await page.getByRole('button', { name: 'Authorize as octocat' }).click()
  await expect(page).toHaveURL(/\/w\/PL$/)
  await page.getByRole('button', { name: 'Account' }).click()
  await expect(page.locator('[data-github-login]')).toHaveText('GitHub: octocat')
  await expect(page.getByRole('menuitem', { name: 'Link GitHub' })).toHaveCount(0)

  const fresh = await browser.newContext({ baseURL: process.env.E2E_BASE_URL })
  const phone = await fresh.newPage()
  await phone.goto('/')
  await expect(phone).toHaveURL(/\/sign-in$/)
  await phone.getByRole('link', { name: 'Sign in with GitHub' }).click()
  await phone.getByRole('button', { name: 'Authorize as octocat' }).click()
  await expect(phone).toHaveURL(/\/w\/PL$/)
  const session = (await (await phone.request.get('/api/session')).json()) as { account: { github_login: string } }
  expect(session.account.github_login).toBe('octocat')
  await fresh.close()
})

test('a GitHub user linked to no account is told so on the sign-in page', async ({ page }) => {
  await page.goto('/sign-in')
  await page.getByRole('link', { name: 'Sign in with GitHub' }).click()
  await page.getByRole('button', { name: 'Authorize as stranger' }).click()
  await expect(page).toHaveURL(/\/sign-in\?unlinked=stranger$/)
  await expect(page.getByRole('alert')).toHaveText(
    'No Punchlist account is linked to GitHub user stranger. Sign in with a token once and link GitHub from the account menu.',
  )
  expect((await page.request.get('/api/session')).status()).toBe(401)
})

test('without the App’s client id and secret the sign-in page offers the token alone', async ({ page }) => {
  await page.route('**/api/auth', (route) => route.fulfill({ json: { github: false } }))
  const options = page.waitForResponse('**/api/auth')
  await page.goto('/sign-in')
  await options
  await expect(page.getByLabel('Token')).toBeVisible()
  await expect(page.getByRole('link', { name: 'Sign in with GitHub' })).toHaveCount(0)
})
