import { defineConfig, devices } from '@playwright/test'

// `pnpm e2e` (scripts/e2e.sh) starts a server on a fresh database and passes its URL and `pl` config.
export default defineConfig({
  testDir: 'e2e',
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? [['github'], ['list']] : 'list',
  use: { baseURL: process.env.E2E_BASE_URL, trace: 'retain-on-failure' },
  projects: [{ name: 'chromium', use: devices['Desktop Chrome'] }],
})
