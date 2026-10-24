-- One sign-in across workspaces (ADR 0011): an account holds a person's token, and the
-- person actor in each workspace is the account's membership there.
CREATE TABLE account (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name       text NOT NULL,
    -- SHA-256 of the bearer token, in hex. The token itself is never stored.
    token_hash text NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE actor ADD COLUMN account_id uuid REFERENCES account (id);
ALTER TABLE actor ADD CONSTRAINT actor_account_membership UNIQUE (workspace_id, account_id);
ALTER TABLE actor ADD CONSTRAINT actor_account_person CHECK (account_id IS NULL OR role = 'person');

-- Each person with a token gets an account named as it, holding the token, so existing tokens
-- and signed-in browsers keep working. Tokenless imported persons get none.
ALTER TABLE actor DROP CONSTRAINT actor_github_identity;
ALTER TABLE account ADD COLUMN actor_id uuid;
INSERT INTO account (name, token_hash, created_at, actor_id)
    SELECT name, token_hash, created_at, id FROM actor
    WHERE role = 'person' AND token_hash IS NOT NULL;
UPDATE actor SET account_id = account.id, token_hash = NULL
    FROM account WHERE account.actor_id = actor.id;
ALTER TABLE account DROP COLUMN actor_id;

-- A person's token is its account's: no person actor holds one.
ALTER TABLE actor ADD CONSTRAINT actor_github_identity
    CHECK ((role = 'github') = (github_login IS NOT NULL)
           AND (role NOT IN ('github', 'person') OR token_hash IS NULL)
           AND (token_hash IS NOT NULL OR role = 'github'
                OR (role = 'person' AND (linear_id IS NOT NULL OR account_id IS NOT NULL))
                OR (role = 'agent' AND kind = 'imported')));
