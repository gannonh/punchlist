#!/usr/bin/env bash
# Postgres in Docker, then the server. Vite joins when web/ exists (Slice 9).
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/env.sh
docker compose up -d --wait db
# Build from the committed .sqlx/ data: the database may not be migrated yet.
SQLX_OFFLINE=true exec cargo run -p punchlist-server -- serve
