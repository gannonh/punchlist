# Sign-in: sessions, GitHub sign-in and linking, and the Origin check

A person signs in at `/sign-in` with a token or with GitHub. Either starts a server-side session, and the `punchlist_session` cookie holds only the session's random id, never a token. A person who signed in with a token links their GitHub user from the account menu; after that `Sign in with GitHub` signs them in. A GitHub user linked to no account is sent back to `/sign-in` with a message and gets no session. The API refuses a write that comes from another origin.

## Sub-features

- `sign-in-token`: `POST /api/session` with a person's token answers 200 with the person and sets `punchlist_session` (`HttpOnly; SameSite=Lax; Path=/`, `Secure` only over https). Any other token answers 401. It starts a `session` row and ends the one the request came with.
- `sign-in-session`: the cookie is a 32-byte base64url id; `session.id` is its SHA-256 hex. A session lasts 30 days from its last use. `DELETE /api/session` deletes the row and clears the cookie (204). A cookie that names no live session answers 401.
- `sign-in-options`: `GET /api/auth` (no token) answers `{"github": true|false}`; `/sign-in` shows `Sign in with GitHub` (`a[data-github-sign-in]`, to `/api/auth/github/start`) above the token field, with an `or` divider, only when it is true.
- `sign-in-github-link`: `Link GitHub` in the account menu (`a[data-link-github]`, to `/api/auth/github/start?link=1`) goes through GitHub's authorize page and back to `/`; the menu then reads `GitHub: <login>` (`[data-github-login]`), and `GET /api/session`'s `account.github_login` is set. A GitHub user already linked to another account answers 409 `conflict`.
- `sign-in-github`: `Sign in with GitHub` for a linked user starts a session and lands on `/`, signed in as the account's person. An unlinked user lands on `/sign-in?unlinked=<login>` with `[data-unlinked]` (`role=alert`): `No Punchlist account is linked to GitHub user <login>. Sign in with a token once and link GitHub from the account menu.` No session row is written.
- `sign-in-state`: `GET /api/auth/github/callback` answers 400 `bad_request` for a missing or wrong `state`, a missing `punchlist_github` cookie, or a link started by another session. `GET /api/auth/github/start` answers 404 `not_found` when GitHub sign-in is off, and `?link=1` answers 401 without a live session.
- `sign-in-origin`: a write (not GET, HEAD or OPTIONS) that carries an `Origin` header or the session cookie needs `Origin` equal to `PUNCHLIST_PUBLIC_URL` when set, else to the request's `Host`; else 403 `refused a write from another origin`. `pl`, runners and GitHub's deliveries send neither.
- `sign-in-account-menu`: `[data-account-menu]` (named `Account`, showing the person's name) on the list, board, Pull requests, inbox and projects screens: `Signed in as <name>`, `GitHub: <login>` or `Link GitHub`, and `Sign out`, which deletes the session and goes to `/sign-in`.

## How to get to it (user POV)

- A signed-out browser opens `/sign-in`. A signed-in one opens the account menu in the header.
- Token sign-in: `URL=$($C sign-in-url "$RUN")`, or paste the token into the field.
- GitHub: with `$C fake-github "$RUN"` run, `Sign in with GitHub` and `Link GitHub` open the fake's authorize page, which has an `Authorize as <login>` button per user (`octocat` id 583231, `stranger` id 9001).

## Driving it with control-punchlist

Preconditions:

- A run started with `RUN=$(PUNCHLIST_VERIFY_HOST=<host> $C up)` and `source /tmp/punchlist-verify/$RUN/run.env`; `pnpm build` done; a headless Chromium on sartre (as in [projects.md](projects.md)). For GitHub: `$C fake-github "$RUN"` once, then `source` run.env again for `FAKE_GITHUB_URL`.

- **Token session.** `$C sign-in-url "$RUN"` in a fresh browser context: it lands on `/` signed in. `$C sql "$RUN" "SELECT count(*) FROM session"` is `1`; the cookie `punchlist_session` is `HttpOnly`, `SameSite=Lax`, and `GET $SERVER_URL/api/session` with it answers the person.
- **Cookie holds no token.** With the cookie value `<v>` from the browser: `$C sql "$RUN" "SELECT count(*) FROM account WHERE token_hash = encode(sha256(convert_to('<v>', 'UTF8')), 'hex')"` is `0`, and `SELECT count(*) FROM session WHERE id = encode(sha256(convert_to('<v>', 'UTF8')), 'hex')` is `1`. Then `$C sql "$RUN" "DELETE FROM session"` (the one write a check may make, standing in for expiry) and the next `curl -s -o /dev/null -w '%{http_code}' -H 'Cookie: punchlist_session=<v>' "$SERVER_URL/api/session"` is `401`.
- **Sign out.** The account menu's `Sign out`: the browser is on `/sign-in`, and `SELECT count(*) FROM session` is `0`.
- **Link.** Signed in with a token, open the account menu, `Link GitHub`, `Authorize as octocat`: back on `/`, the menu shows `GitHub: octocat`, and `$C sql "$RUN" "SELECT github_user_id, github_login FROM account"` is `583231 | octocat`.
- **GitHub sign-in.** In a fresh context, `/sign-in`, `Sign in with GitHub`, `Authorize as octocat`: it lands on `/` signed in as the same person, and a second `session` row exists.
- **Unknown user.** In a fresh context, `Sign in with GitHub`, `Authorize as stranger`: the URL is `/sign-in?unlinked=stranger`, `[data-unlinked]` holds the sentence with `stranger`, and `SELECT count(*) FROM session` did not change.
- **Bad state.** `curl -s -i "$SERVER_URL/api/auth/github/callback?code=code-octocat&state=tampered"` answers 400 `bad_request`, and no `session` row is added.
- **Cross-site write.** `curl -s -o /dev/null -w '%{http_code}' -X POST "$SERVER_URL/api/issues" -H 'Cookie: punchlist_session=<id>' -H 'Origin: https://example.com' -H 'Content-Type: application/json' -d '{"title":"x"}'` is `403` and `SELECT count(*) FROM issue` is unchanged; the same with `-H "Origin: $SERVER_URL"` is `201`.
- **Phone.** At 390 px the sign-in page and the account menu fit: `document.documentElement.scrollWidth <= document.documentElement.clientWidth`.
- **E2E.** `web/e2e/sign-in.spec.ts` drives link, sign-in, unknown user and sign-out against the fake.

## Gotchas

- The cookie is `SameSite=Lax`, not Strict: the session set on GitHub's redirect back must be kept. The Origin check is the CSRF defence, so a cross-site `POST` check needs a cookie or an `Origin` header; plain `curl` with neither is allowed, as `pl` is.
- A cookie from before sessions held the raw token and now answers 401. Sign in again.
- The browser must reach the fake: it is redirected to `FAKE_GITHUB_URL`. Start the run with `PUNCHLIST_VERIFY_HOST` set to an address the browser can reach so the server and the fake bind the same host; `127.0.0.1` suits a headless browser on sartre.
- `$C restart "$RUN"` keeps the fake's four variables; `$C down "$RUN"` stops the fake. Its log is `verify-artifacts/<run>/fake-github.log`, which prints each token exchange's body (the secret redacted).
- The server sends GitHub no `redirect_uri`, so GitHub sends every sign-in to the App's registered Callback URL. The fake does the same with the run's `$SERVER_URL/api/auth/github/callback`, so the browser comes back to the address the run started on, whatever address it started the flow from.
- The Origin check compares against the request's `Host` unless `PUNCHLIST_PUBLIC_URL` is set, so a check that uses a different address for the same server (an `Origin` of `localhost` against a run on a Tailscale address) is refused.
- `pnpm e2e` runs `fake-github` after `up`, so GitHub sign-in is on in that run; a plain `$C up` run has it off and `/sign-in` shows the token field alone.
