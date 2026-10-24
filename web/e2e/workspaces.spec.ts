import { execFileSync } from 'node:child_process'
import { expect, test } from '@playwright/test'
import { bearer, ok } from './support'

const server = new URL('../../target/debug/punchlist-server', import.meta.url).pathname

// A workspace of its own account, so the run's person stays in one workspace for the other specs.
function bootstrap(name: string, prefix: string, account?: string): string {
  const database = process.env.E2E_DATABASE_URL
  if (!database) throw new Error('E2E_DATABASE_URL is not set; run `pnpm e2e`')
  const args = ['--database-url', database, 'bootstrap', '--person', 'Workspace Person', '--workspace', name]
  args.push('--prefix', prefix, '--repository', `gannonh/e2e-${prefix.toLowerCase()}`)
  if (account) args.push('--account', account)
  return execFileSync(server, args, { stdio: ['ignore', 'pipe', 'ignore'] }).toString()
}

test('one sign-in reaches both workspaces of the account, and the menu switches between them', async ({
  page,
  request,
}) => {
  const token = /^token = "(.*)"$/m.exec(bootstrap('Alpha', 'AL'))?.[1] ?? ''
  const session = await ok<{ account: { id: string } }>(await request.get('/api/session', { headers: bearer(token) }))
  bootstrap('Groundwork', 'GW', session.account.id)
  const workspaces = await ok<{ id: string; issue_prefix: string }[]>(
    await request.get('/api/workspaces', { headers: bearer(token) }),
  )
  for (const { id, issue_prefix } of workspaces)
    await ok(
      await request.post(`/api/workspaces/${id}/issues`, {
        headers: bearer(token),
        data: { title: `First in ${issue_prefix}`, body: '' },
      }),
    )

  await page.goto('/sign-in')
  await page.getByLabel('Token').fill(token)
  await page.getByRole('button', { name: 'Sign in' }).click()
  await expect(page).toHaveURL(/\/w\/AL$/)
  await expect(page.locator('[data-issue]').first()).toHaveAttribute('data-issue', 'AL-1')

  await page.getByRole('button', { name: 'Workspace' }).click()
  await expect(page.getByRole('menuitemradio')).toHaveText([/^AlphaAL$/, /^GroundworkGW$/])
  await expect(page.getByRole('menuitemradio', { name: /Alpha/ })).toHaveAttribute('aria-checked', 'true')
  await page.getByRole('menuitemradio', { name: /Groundwork/ }).click()

  await expect(page).toHaveURL(/\/w\/GW$/)
  await expect(page.getByRole('button', { name: 'Workspace' })).toContainText('Groundwork')
  await expect(page.locator('[data-issue]')).toHaveCount(1)
  await expect(page.locator('[data-issue]').first()).toHaveAttribute('data-issue', 'GW-1')

  await page.goto('/')
  await expect(page).toHaveURL(/\/w\/GW$/)
})
