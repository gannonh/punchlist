-- `pl auth login` (PL-170): a person approves a terminal's login in the browser, which gives
-- `pl` a session of its own, sent as its bearer value. A browser's session is only ever a
-- cookie, and a CLI session only ever a bearer value.
ALTER TABLE session ADD COLUMN kind text NOT NULL DEFAULT 'browser'
    CHECK (kind IN ('browser', 'cli'));

-- A login waiting for a person. `pl` polls it with a secret whose SHA-256 is `secret_hash`;
-- an approved one answers the poll once with a new CLI session, then is `used`.
CREATE TABLE cli_login (
    -- The 8 letters the person sees in the terminal and on the approval page.
    code        text PRIMARY KEY,
    secret_hash text NOT NULL UNIQUE,
    -- The user agent `pl` asked with, which names its host.
    user_agent  text,
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,
    state       text NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'approved', 'denied', 'used')),
    -- Who approved it.
    account_id  uuid REFERENCES account (id) ON DELETE CASCADE,
    CHECK ((state IN ('approved', 'used')) = (account_id IS NOT NULL))
);
