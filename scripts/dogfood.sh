#!/usr/bin/env bash
# `docker compose` for the dogfood instance on sartre (docs/dogfood.md): its address and ports,
# and the dogfood GitHub App's secrets under the names the server reads. The Punchlist
# Environment keeps the plain GITHUB_* names for the sandbox App.
#   scripts/dogfood.sh up -d --build
set -euo pipefail
cd "$(dirname "$0")/.."
export PUNCHLIST_HOST=127.0.0.1 PUNCHLIST_PORT=7880 PUNCHLIST_DB_PORT=5440
exec with-env bash -c 'GITHUB_APP_ID=$DOGFOOD_GITHUB_APP_ID GITHUB_APP_PRIVATE_KEY=$DOGFOOD_GITHUB_APP_PRIVATE_KEY GITHUB_WEBHOOK_SECRET=$DOGFOOD_GITHUB_WEBHOOK_SECRET exec docker compose "$@"' dogfood "$@"
