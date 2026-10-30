import { execFileSync } from 'node:child_process'
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

const server = new URL('../../target/debug/punchlist-server', import.meta.url).pathname

/** Bootstraps a workspace on the repository `gannonh/e2e-<prefix>`, in a new account or in `account`; prints its `pl` config. */
export function bootstrap(name: string, prefix: string, account?: string): string {
  const database = process.env.E2E_DATABASE_URL
  if (!database) throw new Error('E2E_DATABASE_URL is not set; run `pnpm e2e`')
  const args = ['--database-url', database, 'bootstrap', '--person', 'Workspace Person', '--workspace', name]
  args.push('--prefix', prefix, '--repository', `gannonh/e2e-${prefix.toLowerCase()}`)
  if (account) args.push('--account', account)
  return execFileSync(server, args, { stdio: ['ignore', 'pipe', 'ignore'] }).toString()
}

export interface Workspace {
  prefix: string
  token: string
}

/**
 * A workspace of its own account, for a test that drives a runner: a claim takes the first work in its workspace's
 * claim order, whoever created it, so a runner in the shared PL workspace would take another test's runs.
 */
export function workspace(): Workspace {
  const prefix = `R${randomUUID().replaceAll('-', '').slice(0, 9).toUpperCase()}`
  const token = /^token = "(.*)"$/m.exec(bootstrap(`Runs ${prefix}`, prefix))?.[1]
  if (!token) throw new Error(`no token for workspace ${prefix}`)
  return { prefix, token }
}

/** The repository name of the workspace `id` is in: `workspace()`'s, or the run's sandbox for PL. */
export function repositoryOf(id: string): string {
  const prefix = id.slice(0, id.lastIndexOf('-'))
  return prefix === 'PL' ? 'punchlist-sandbox' : `e2e-${prefix.toLowerCase()}`
}

export interface Claim {
  run_id: string
  attempt: number
  agent_token: string
  issue: { id: string; status: string }
}

/** The runner's claim of `id`, the only work in its workspace. A claim waits up to 10 s; 204 is nothing yet. */
export async function claim(request: APIRequestContext, runner: string, id: string): Promise<Claim> {
  for (let tries = 0; tries < 3; tries++) {
    const response = await request.post('/api/runs/claim', { headers: bearer(runner), data: { wait_seconds: 10 } })
    if (response.status() === 204) continue
    const claimed = await ok<Claim>(response)
    expect(claimed.issue.id).toBe(id)
    return claimed
  }
  throw new Error(`no claim of ${id} in 30 s`)
}

export async function ok<T>(response: Awaited<ReturnType<APIRequestContext['post']>>): Promise<T> {
  expect(response.ok(), `${response.url()} answered ${String(response.status())}: ${await response.text()}`).toBe(true)
  return (await response.json()) as T
}

export const fixture = (name: string): unknown =>
  JSON.parse(
    readFileSync(new URL(`../../crates/punchlist-server/tests/fixtures/github/${name}.json`, import.meta.url), 'utf8'),
  )

/** A delivery to the webhook on the run's repository, signed as GitHub signs one. */
export async function deliver(
  request: APIRequestContext,
  event: string,
  payload: { repository: { name: string } },
  repository = 'punchlist-sandbox',
) {
  const secret = process.env.E2E_WEBHOOK_SECRET_FILE
  if (!secret) throw new Error('E2E_WEBHOOK_SECRET_FILE is not set; run `pnpm e2e`')
  payload.repository.name = repository
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
    pull_request: { number: number; html_url: string; head: { ref: string; sha: string } }
  }
  payload.pull_request.number = head.number
  payload.pull_request.html_url = `https://github.com/gannonh/${repositoryOf(id)}/pull/${String(head.number)}`
  payload.pull_request.head.ref = `feature/${id.toLowerCase()}-live`
  payload.pull_request.head.sha = head.sha
  await deliver(request, 'pull_request', payload, repositoryOf(id))
}

export async function signIn(page: Page, { prefix, token }: Workspace = { prefix: 'PL', token: personToken() }) {
  await page.goto('/')
  await expect(page).toHaveURL(/\/sign-in$/)
  await page.getByLabel('Token').fill(token)
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page).toHaveURL(new RegExp(`/w/${prefix}$`))
}

export interface Inbox {
  items: { issue: { id: string; status: string }; unread: boolean }[]
  unread: number
}

export const inbox = async (request: APIRequestContext, token = personToken()) =>
  ok<Inbox>(await request.get('/api/inbox', { headers: bearer(token) }))

// Other spec files move issues meanwhile, so the header is checked against the inbox as the server has it then. The
// header trails the server by up to 600 ms (the stream's 500 ms poll and the inbox's 100 ms wait), so the check
// samples every 100 ms until the two agree between other workers' moves (PL-178).
export async function headerMatches(page: Page, request: APIRequestContext, token = personToken()) {
  await expect
    .poll(
      async () => {
        const [shown, now] = [await page.locator('[data-inbox-count]').textContent(), await inbox(request, token)]
        return shown === String(now.unread)
      },
      { intervals: [100] },
    )
    .toBe(true)
}
