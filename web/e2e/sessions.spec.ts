import { expect, test } from '@playwright/test'
import { signIn } from './support'

const IPHONE =
  'Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1'

// Other spec files sign the same person in at the same time, so this one ends only the phone's session, by its id,
// and never `Sign out everywhere else`.
test('a person signs a phone out from the Sessions list and the phone lands on the sign-in page', async ({
  page,
  browser,
}) => {
  const phoneContext = await browser.newContext({ baseURL: process.env.E2E_BASE_URL, userAgent: IPHONE })
  const phone = await phoneContext.newPage()
  await signIn(phone)
  const phoneSessions = (await (await phone.request.get('/api/sessions')).json()) as { id: string; current: boolean }[]
  const phoneId = phoneSessions.find((s) => s.current)?.id
  expect(phoneId).toBeDefined()

  await signIn(page)
  await page.getByRole('button', { name: 'Account' }).click()
  await page.getByRole('menuitem', { name: 'Sessions' }).click()
  const dialog = page.getByRole('dialog', { name: 'Sessions' })
  const phoneRow = dialog.locator(`[data-session="${String(phoneId)}"]`)
  await expect(phoneRow.locator('[data-device]')).toHaveText('Safari on iPhone')
  await expect(phoneRow).toContainText(/Last used \d+s ago/)
  await expect(dialog.locator('li:has([data-current])')).toHaveCount(1)
  await expect(dialog.locator('li:has([data-current]) [data-device]')).toHaveText('Chrome on Windows')
  await expect(dialog.locator('[data-current]')).toHaveText('This device')

  // The row drops before the server answers, so the phone's read waits for the DELETE.
  const revoked = page.waitForResponse(
    (response) => response.request().method() === 'DELETE' && response.url().endsWith(`/api/sessions/${String(phoneId)}`),
  )
  await phoneRow.getByRole('button', { name: /^Sign out Safari on iPhone/ }).click()
  await expect(phoneRow).toHaveCount(0)
  expect((await revoked).status()).toBe(204)
  expect((await phone.request.get('/api/session')).status()).toBe(401)

  // The phone's next request in the app answers 401 and shows the sign-in page, with no reload. That request may be a
  // read another spec's event starts before this one opens the inbox, so the key waits for no link to be there.
  await phone.keyboard.press('i')
  await expect(phone).toHaveURL(/\/sign-in$/)
  await expect(phone.getByLabel('Token')).toBeVisible()
  // This browser is still signed in.
  expect((await page.request.get('/api/session')).status()).toBe(200)
  await phoneContext.close()
})
