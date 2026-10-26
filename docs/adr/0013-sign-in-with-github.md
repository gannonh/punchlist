# 13. Sign in with GitHub, with server-side sessions

Date: 2026-10-26. Status: accepted.

## Context

A person signs in to the web app today by pasting a token (ADR 0011). The server keeps that raw account token in the `punchlist_session` cookie, `SameSite=Strict`. Three things are wrong with that:

- A token in a cookie is a long-lived secret in the browser. Signing out clears the cookie but revokes nothing, and the server cannot end one browser's sign-in without rotating the token `pl` and the runners use.
- Pasting a token is the only way in. Every person already has a GitHub login, and each workspace already has a GitHub App installed on its repository.
- `SameSite=Strict` is the only defence against cross-site writes, and it blocks the cookie on the redirect back from GitHub, so a session set there would not be kept.

Options for the identity provider:

- **(a) The workspace's GitHub App, as a user authorization.** The App has an OAuth client id and secret. No new registration; GitHub shows one App for everything Punchlist does.
- **(b) A separate GitHub OAuth App.** A second registration, secret and callback to keep. It gains nothing over (a) for reading the user's id and login.
- **(c) Another provider or passwords.** More to build and operate, and nobody asked for it.

Options for the session:

- **(d) Keep the token in the cookie.** GitHub sign-in would then have to hand the browser the account's token. Rejected.
- **(e) A server-side session row, with a random id in the cookie.** The server can end it, and the cookie is worth nothing outside this server.

## Decision

- **Identity: (a).** The server redirects to `/login/oauth/authorize` with the App's client id, reads the GitHub user's id and login with the code, and keeps nothing else. The user's GitHub token is never stored.
- **Sessions: (e).** `session` holds the SHA-256 of the cookie's random value, the account, creation and last-use times, expiry and user agent. A session lasts 30 days from its last use. `DELETE /api/session` deletes the row. Token sign-in (`POST /api/session`) starts a session too, so the cookie never holds a token.
- **Tokens stay.** `pl`, runners and agents authenticate with their tokens as before. Only the browser's cookie changes.
- **No self sign-up in this slice.** A GitHub user signs in only when linked to an existing account: `account.github_user_id` is unique, and a person links it from the account menu while signed in with a token. An unlinked GitHub user is sent back to `/sign-in` with a message and no session. Inviting people stays out of scope (ADR 0011).
- **The cookie: `SameSite=Lax`.** It supersedes ADR 0011's "The cookie: unchanged" bullet, which said `SameSite=Strict` and a token. Lax is needed so the session set on GitHub's redirect back is kept. `HttpOnly; Path=/`, and `Secure` over https, stay.
- **CSRF: an Origin check instead of `SameSite=Strict`.** Every API write that carries an `Origin` header or the session cookie must have `Origin` equal to `PUNCHLIST_PUBLIC_URL` when set, else to the request's host. `pl`, runners and GitHub's deliveries send neither and are unaffected. The OAuth `state` is a random value in its own short-lived cookie, and linking also needs the session that started it.
- **Configuration.** `GITHUB_APP_CLIENT_ID`, `GITHUB_APP_CLIENT_SECRET` and `PUNCHLIST_PUBLIC_URL`. Without the client id and secret GitHub sign-in is off and the sign-in page shows the token field alone.

## Consequences

- A browser that held the old token cookie is signed out once at the upgrade and signs in again.
- A person can end a browser's sign-in without touching the token `pl` uses. A session UI is not built yet.
- Browsers must use `PUNCHLIST_PUBLIC_URL`, since a write from another origin is refused.
- GitHub sign-in depends on the App's callback URL, `<origin>/api/auth/github/callback`, being set on the App.
- Self sign-up, session listing and revocation from the web app, and a second provider remain open.
