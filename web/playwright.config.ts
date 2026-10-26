import { defineConfig, devices } from '@playwright/test'

// `pnpm e2e` (scripts/e2e.sh) starts a server on a fresh database and passes its URL and `pl` config.
export default defineConfig({
  testDir: 'e2e',
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? [['github'], ['list']] : 'list',
  use: { baseURL: process.env.E2E_BASE_URL, trace: 'retain-on-failure' },
  projects: [
    {
      name: 'chromium',
      // Playwright's default --disable-dev-shm-usage puts renderer shared memory in /tmp, a quota-limited
      // tmpfs on sartre; renderers crash when the quota fills (PL-157), so keep it in /dev/shm.
      use: { ...devices['Desktop Chrome'], launchOptions: { ignoreDefaultArgs: ['--disable-dev-shm-usage'] } },
    },
  ],
})
