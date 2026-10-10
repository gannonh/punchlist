# API document and auth

The server publishes its OpenAPI document at `GET /api/openapi.json` (no token needed) and requires a bearer token on every `/api/issues` route. `pl` reads its server URL and token from `~/.config/punchlist/config.toml` or the file in `PUNCHLIST_CONFIG`.

## Sub-features

- `openapi` lists 16 paths: `/api/github/webhook`, `/api/issues`, `/api/issues/{id}`, `/api/issues/{id}/comments`, `/api/issues/{id}/events`, `/api/issues/{id}/pull-requests`, `/api/issues/{id}/runs`, `/api/issues/{id}/transitions`, `/api/pull-requests/unlinked`, `/api/runners`, `/api/runners/{id}/heartbeat`, `/api/runs/claim`, `/api/runs/{id}/finish`, `/api/runs/{id}/log`, `/api/workflow` and `/api/workflow/schema.json`. It does not list `/api/openapi.json` itself.
- `auth-required` answers `401` with `{"code":"unauthorized"}` for a missing or unknown token.
- `pl-config-missing` exits `1` with `error: cannot read <path>; it needs `server_url` and `token` ...`.
- `not-found` answers `404` with `issue PL-9 not found`.

## How to get to it (user POV)

- `curl $SERVER_URL/api/openapi.json`
- Any `/api/issues` request with no or a wrong `Authorization: Bearer` header.
- `pl` with no config file.

## Driving it with control-punchlist

Preconditions:

- Baseline run `$RUN`; `source /tmp/punchlist-verify/$RUN/run.env`.

- **Document.** Run `curl -s "$SERVER_URL/api/openapi.json" | python3 -c 'import json,sys; print(sorted(json.load(sys.stdin)["paths"]))'`. Prints the 16 paths above.
- **No token.** Run `curl -s -w ' %{http_code}' "$SERVER_URL/api/issues"`. Body `{"code":"unauthorized","message":"missing or unknown bearer token"}`, status `401`.
- **Missing config.** Run `PUNCHLIST_CONFIG=/nonexistent/config.toml target/debug/pl issue list; echo $?`. Stderr starts `error: cannot read /nonexistent/config.toml; it needs `server_url` and `token``, exit `1`.
- **Unknown issue.** Run `$C pl "$RUN" issue show PL-9`. Exit `1`, stderr `error: issue PL-9 not found (404 Not Found)`.

## Gotchas

- Tokens are stored only as SHA-256 hashes; a lost token cannot be recovered or rotated. Start a new run (`$C up`) to get one.
- `bootstrap` refuses a second workspace with the same prefix (`PL`), so run it once per database.
