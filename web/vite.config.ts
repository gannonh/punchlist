import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vite'
import viteReact from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// `pnpm dev` proxies /api to the server it started (scripts/dev.sh), so the app and the API share one
// origin and the SameSite=Strict session cookie works as it does when the server serves web/dist.
const api = `http://${process.env.PUNCHLIST_BIND ?? '127.0.0.1:7878'}`

export default defineConfig({
  resolve: {
    tsconfigPaths: true,
    // tsconfig.json's `cn` path, which Vite skips for an installed package name.
    alias: [{ find: /^cn$/, replacement: fileURLToPath(new URL('./src/lib/utils.ts', import.meta.url)) }],
  },
  server: { proxy: { '/api': { target: api } } },
  plugins: [tailwindcss(), viteReact()],
})
