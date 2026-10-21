#!/usr/bin/env bash
# The self-host stack end to end on throwaway volumes: build the image, run the Slice 1 flow
# through `pl`, then check the data survives a restart, and a backup restored into new volumes.
set -euo pipefail
cd "$(dirname "$0")/.."
free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
export COMPOSE_PROJECT_NAME="punchlist-e2e-$$" PUNCHLIST_HOST=127.0.0.1
PUNCHLIST_PORT=$(free_port) PUNCHLIST_DB_PORT=$(free_port)
export PUNCHLIST_PORT PUNCHLIST_DB_PORT
URL="http://127.0.0.1:$PUNCHLIST_PORT"
DIR=$(mktemp -d)
trap 'docker compose down -v >/dev/null 2>&1; rm -rf "$DIR"' EXIT
export PUNCHLIST_CONFIG="$DIR/config.toml"

SQLX_OFFLINE=true cargo build -q -p punchlist-cli
pl() { target/debug/pl "$@"; }
expect() {
  local label=$1 want=$2 out
  shift 2
  out=$("$@" 2>&1) || true
  if ! grep -qF -- "$want" <<<"$out"; then
    printf 'FAIL %s: no "%s" in\n%s\n' "$label" "$want" "$out"
    docker compose logs server | tail -40
    exit 1
  fi
  echo "ok   $label"
}
wait_up() {
  for _ in $(seq 60); do curl -sf -o /dev/null "$URL/api/openapi.json" && return; sleep 1; done
  echo "FAIL the server did not answer at $URL"; docker compose logs server | tail -40; exit 1
}
check() {
  expect "$1: issue status" "Status: Todo" pl issue show PL-1
  expect "$1: issue list" "Survives restarts" pl issue list
  expect "$1: proof record" "e2e proof" pl proof list PL-1
  expect "$1: proof file" "proof bytes" pl_proof_file
  expect "$1: web app" '<div id="root">' curl -s "$URL/"
}
pl_proof_file() {
  local id
  id=$(docker compose exec -T db psql -U punchlist -tAc 'SELECT id FROM proof')
  curl -s -H @<(sed -n 's/^token = "\(.*\)"$/Authorization: Bearer \1/p' "$PUNCHLIST_CONFIG") "$URL/api/proof/$id"
}

docker compose up -d --build --wait
wait_up
(umask 077 && docker compose exec -T server punchlist-server bootstrap --person "E2E Person" \
  --server-url "$URL" >"$PUNCHLIST_CONFIG")
expect "create" "PL-1" pl issue create --title "Survives restarts" --body "Body"
expect "move" "Todo" pl issue move PL-1 todo
echo "proof bytes" >"$DIR/proof.txt"
expect "proof attach" "e2e proof" pl proof attach PL-1 "$DIR/proof.txt" --title "e2e proof"
check "first start"

docker compose restart
wait_up
check "after restart"

docker compose exec -T db pg_dump -U punchlist -Fc punchlist >"$DIR/db.dump"
docker compose exec -T server tar -C /var/lib/punchlist -cf - proof >"$DIR/proof.tar"
docker compose down -v
docker compose up -d --wait db
docker compose exec -T db pg_restore -U punchlist -d punchlist --clean --if-exists <"$DIR/db.dump"
docker compose run --rm -T --no-deps --entrypoint tar server -C /var/lib/punchlist -xf - <"$DIR/proof.tar"
docker compose up -d --wait
wait_up
check "after restore"
