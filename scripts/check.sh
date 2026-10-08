#!/usr/bin/env bash
# Everything a pull request must pass. CI runs this same script.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/env.sh

step() { printf '\n==> %s\n' "$*"; }

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
