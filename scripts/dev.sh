#!/usr/bin/env bash
# Postgres in Docker, the server, and Vite's dev server proxying /api to it (web/vite.config.ts).
# PUNCHLIST_DEV_VITE=0 runs the server alone, serving the build in web/dist (`pnpm build`).
# PUNCHLIST_VITE_HOST binds Vite elsewhere, such as sartre's Tailscale address (`tailscale ip -4`).
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/env.sh
docker compose up -d --wait db
if [ "${PUNCHLIST_DEV_VITE:-1}" != 0 ]; then
  pnpm --filter @punchlist/web exec vite --host "${PUNCHLIST_VITE_HOST:-127.0.0.1}" --port "${PUNCHLIST_VITE_PORT:-5173}" &
  trap 'kill $! 2>/dev/null || true' EXIT
fi
# Build from the committed .sqlx/ data: the database may not be migrated yet.
SQLX_OFFLINE=true cargo run -p punchlist-server -- serve
