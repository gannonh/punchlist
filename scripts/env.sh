# Sourced by the other scripts: local development defaults. Not secrets; the
# database only listens on 127.0.0.1.
export DATABASE_URL="${DATABASE_URL:-postgres://punchlist:punchlist@127.0.0.1:${PUNCHLIST_DB_PORT:-5433}/punchlist}"
export PUNCHLIST_BIND="${PUNCHLIST_BIND:-127.0.0.1:7878}"
