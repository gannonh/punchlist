#!/usr/bin/env bash
# Regenerate .sqlx/ after changing a query or a migration.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/env.sh
docker compose up -d --wait db
cargo sqlx migrate run --source crates/punchlist-server/migrations
cargo sqlx prepare --workspace -- --all-targets
