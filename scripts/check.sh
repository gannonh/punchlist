#!/usr/bin/env bash
# Everything a pull request must pass. CI runs this same script.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/env.sh

step() { printf '\n==> %s\n' "$*"; }

step "web dependencies"
pnpm install --frozen-lockfile

step "web/src/api is current (pnpm gen:api)"
pnpm gen:api >/dev/null
git diff --exit-code --stat -- web/src/api || { echo "web/src/api is stale: run pnpm gen:api and commit it" >&2; exit 1; }

step "web typecheck, lint and unit tests"
pnpm --filter @punchlist/web typecheck
pnpm --filter @punchlist/web lint
pnpm --filter @punchlist/web test
node --test scripts/backfill-blockers.test.mjs

step "rustfmt"
cargo fmt --all --check

step "clippy"
SQLX_OFFLINE=true cargo clippy --workspace --all-targets --locked -- -D warnings

step "punchlist-core builds for wasm32"
cargo check -p punchlist-core --target wasm32-unknown-unknown --locked

step "postgres"
docker compose up -d --wait db

step "cargo test"
SQLX_OFFLINE=true cargo test --workspace --locked

step ".sqlx is current"
cargo sqlx migrate run --source crates/punchlist-server/migrations
cargo sqlx prepare --workspace --check -- --all-targets
