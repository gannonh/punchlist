-- Sign-in with GitHub and server-side sessions (ADR 0013). The browser's cookie holds a random
-- session id; only its SHA-256 is stored, as for tokens.
CREATE TABLE session (
    -- SHA-256 of the cookie's value, in hex.
    id           text PRIMARY KEY,
    account_id   uuid NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz NOT NULL DEFAULT now(),
    -- 30 days from the last use.
    expires_at   timestamptz NOT NULL,
    user_agent   text
);

-- The GitHub user an account signs in as. The user's token is never stored.
ALTER TABLE account ADD COLUMN github_user_id bigint UNIQUE;
ALTER TABLE account ADD COLUMN github_login text;
