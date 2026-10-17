#!/usr/bin/env bash
# Playwright against the built app, served by a server on a fresh database (verify-punchlist's `up`).
set -euo pipefail
cd "$(dirname "$0")/.."
C=.claude/skills/verify-punchlist/bin/control-punchlist
pnpm build
RUN=$($C up "e2e-$(date +%Y%m%d-%H%M%S)-$$")
trap '$C down "$RUN"' EXIT
# shellcheck disable=SC1090
source "${TMPDIR:-/tmp}/punchlist-verify/$RUN/run.env"
E2E_BASE_URL="$SERVER_URL" E2E_CONFIG="$CONFIG" pnpm --filter @punchlist/web e2e "$@"
