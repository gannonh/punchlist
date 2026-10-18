# API document and auth

The server publishes its OpenAPI document at `GET /api/openapi.json` (no token needed) and requires a token on every other route except `POST /api/session` and the signature-checked `POST /api/github/webhook`: a bearer token, or for a person without an `Authorization` header the `punchlist_session` cookie ([web-app.md](web-app.md)). `pl` reads its server URL and token from `~/.config/punchlist/config.toml` or the file in `PUNCHLIST_CONFIG`.

## Sub-features

- `openapi` lists every API path, from `/api/actors` to `/api/workspaces/{id}/events`; `/api/openapi.json` itself is not among them.
- `auth-required` answers `401` with `{"code":"unauthorized"}` for a missing or unknown token.
- `pl-config-missing` exits `1` with `error: cannot read <path>; it needs `server_url` and `token` ...`.
- `not-found` answers `404` with `issue PL-9 not found`.
- `other-workspace`: any token on another workspace's issue answers `403` (`issue PL-1 is in another workspace`), and the server logs `workspace mismatch`. [issue-placement.md](issue-placement.md) makes the second workspace.

## How to get to it (user POV)

- `curl $SERVER_URL/api/openapi.json`
- Any `/api/issues` request with no or a wrong `Authorization: Bearer` header.
- `pl` with no config file.

## Driving it with control-punchlist

Preconditions:

- Baseline run `$RUN`; `source /tmp/punchlist-verify/$RUN/run.env`.

- **Document.** Run `curl -s "$SERVER_URL/api/openapi.json" | python3 -c 'import json,sys; print(sorted(json.load(sys.stdin)["paths"]))'`. The list includes `/api/actors`, `/api/issues`, `/api/issues/{id}`, `/api/issues/{id}/events`, `/api/issues/{id}/transitions`, `/api/proof/{id}`, `/api/projects`, `/api/session` and `/api/workflow`, and not `/api/openapi.json`. Assert membership, not the exact list: it grows with each slice.
- **No token.** Run `curl -s -w ' %{http_code}' "$SERVER_URL/api/issues"`, then the same with `-H 'Authorization: Bearer plt_bad'`. Both: body `{"code":"unauthorized","message":"missing or unknown bearer token"}`, status `401`.
- **Missing config.** Run `PUNCHLIST_CONFIG=/nonexistent/config.toml target/debug/pl issue list; echo $?`. Stderr `error: cannot read /nonexistent/config.toml; it needs `server_url` and `token` (punchlist-server bootstrap prints both): No such file or directory (os error 2)`, exit `1`.
- **Unknown issue.** Run `$C pl "$RUN" issue show PL-9`. Exit `1`, stderr `error: issue PL-9 not found (404 Not Found)`.

## Gotchas

- Tokens are stored only as SHA-256 hashes; a lost person token cannot be recovered, only replaced by bootstrapping again.
- `bootstrap` refuses a second workspace with the same prefix (`PL`): `duplicate key value violates unique constraint "workspace_issue_prefix_key"`. A second workspace in the run's database needs `--prefix`, such as `OT`.
