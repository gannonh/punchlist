-- Invites (PL-169): a person of a workspace names a GitHub login, and that GitHub user's next
-- sign-in with GitHub makes them a person there, creating their account when they have none.
-- citext is a trusted extension, as pg_trgm is.
CREATE EXTENSION IF NOT EXISTS citext;

CREATE TABLE invite (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspace (id),
    github_login citext NOT NULL,
    invited_by   uuid NOT NULL REFERENCES actor (id),
    created_at   timestamptz NOT NULL DEFAULT now(),
    accepted_at  timestamptz,
    revoked_at   timestamptz,
    CHECK (accepted_at IS NULL OR revoked_at IS NULL)
);

-- One open invite per login and workspace.
CREATE UNIQUE INDEX invite_open ON invite (workspace_id, github_login)
    WHERE accepted_at IS NULL AND revoked_at IS NULL;

-- An account an invite created signs in with GitHub alone, so it holds no token.
ALTER TABLE account ALTER COLUMN token_hash DROP NOT NULL;
