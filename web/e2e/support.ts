import { createHmac, randomUUID } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { expect, type APIRequestContext, type Page } from '@playwright/test'

// The person token `bootstrap` wrote into the run's `pl` config.
export function personToken(): string {
  const config = process.env.E2E_CONFIG
  if (!config) throw new Error('E2E_CONFIG is not set; run `pnpm e2e`')
  const token = /^token = "(.*)"$/m.exec(readFileSync(config, 'utf8'))?.[1]
  if (!token) throw new Error(`no token in ${config}`)
  return token
}

export const bearer = (token: string) => ({ Authorization: `Bearer ${token}` })

export async function ok<T>(response: Awaited<ReturnType<APIRequestContext['post']>>): Promise<T> {
  expect(response.ok(), `${response.url()} answered ${String(response.status())}: ${await response.text()}`).toBe(true)
  return (await response.json()) as T
}

export const fixture = (name: string): unknown =>
  JSON.parse(
    readFileSync(new URL(`../../crates/punchlist-server/tests/fixtures/github/${name}.json`, import.meta.url), 'utf8'),
  )

/** A delivery to the webhook on the run's repository, signed as GitHub signs one. */
export async function deliver(request: APIRequestContext, event: string, payload: { repository: { name: string } }) {
  const secret = process.env.E2E_WEBHOOK_SECRET_FILE
  if (!secret) throw new Error('E2E_WEBHOOK_SECRET_FILE is not set; run `pnpm e2e`')
  payload.repository.name = 'punchlist-sandbox'
  const body = JSON.stringify(payload)
  const signature = createHmac('sha256', readFileSync(secret, 'utf8')).update(body).digest('hex')
  const response = await request.post('/api/github/webhook', {
    headers: {
      'content-type': 'application/json',
      'x-github-event': event,
      'x-github-delivery': randomUUID(),
      'x-hub-signature-256': `sha256=${signature}`,
    },
    data: body,
  })
  expect(response.status()).toBe(202)
}

/** A `pull_request` delivery for the issue's branch: number 7 on the fixture's head commit unless given. */
export async function deliverPullRequest(
  request: APIRequestContext,
  id: string,
  action: 'opened' | 'ready_for_review',
  head: { number: number; sha: string } = { number: 7, sha: '6dcb09b5b57875f334f61aebed695e2e4193db5e' },
) {
  const payload = fixture(`pull_request_${action}`) as {
    repository: { name: string }
    pull_request: { number: number; head: { ref: string; sha: string } }
  }
  payload.pull_request.number = head.number
  payload.pull_request.head.ref = `feature/${id.toLowerCase()}-live`
  payload.pull_request.head.sha = head.sha
  await deliver(request, 'pull_request', payload)
}

export async function signIn(page: Page) {
  await page.goto('/')
  await expect(page).toHaveURL(/\/sign-in$/)
  await page.getByLabel('Token').fill(personToken())
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page).toHaveURL(/\/w\/PL$/)
}
