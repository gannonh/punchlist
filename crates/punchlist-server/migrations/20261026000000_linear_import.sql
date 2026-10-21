-- Slice 11: `pl import linear` imports a Linear project's issues, comments, project, milestones
-- and the people who wrote them. Each imported row keeps its Linear id, the key a rerun finds it
-- by, unique within what it belongs to so two workspaces can import the same project.

ALTER TABLE issue
    ADD COLUMN linear_id text,
    -- Such as KAT-12, and the issue's page in Linear.
    ADD COLUMN linear_identifier text,
    ADD COLUMN linear_url text,
    -- Linear's `updatedAt` as last imported: a rerun updates only an issue Linear changed since.
    ADD COLUMN linear_updated_at timestamptz;
CREATE UNIQUE INDEX issue_linear_id ON issue (workspace_id, linear_id) WHERE linear_id IS NOT NULL;

ALTER TABLE comment ADD COLUMN linear_id text;
CREATE UNIQUE INDEX comment_linear_id ON comment (issue_id, linear_id) WHERE linear_id IS NOT NULL;

ALTER TABLE project ADD COLUMN linear_id text;
CREATE UNIQUE INDEX project_linear_id ON project (workspace_id, linear_id) WHERE linear_id IS NOT NULL;

ALTER TABLE milestone ADD COLUMN linear_id text;
CREATE UNIQUE INDEX milestone_linear_id ON milestone (project_id, linear_id) WHERE linear_id IS NOT NULL;

-- A Linear user is a person with no token: they never call the API. Comments signed `(agent)`,
-- and those with no user, belong to one agent per workspace of kind `imported`, owned by the
-- person who ran the first import, also with no token.
ALTER TABLE actor ADD COLUMN linear_id text;
CREATE UNIQUE INDEX actor_linear_id ON actor (workspace_id, linear_id) WHERE linear_id IS NOT NULL;
CREATE UNIQUE INDEX actor_imported_agent ON actor (workspace_id) WHERE kind = 'imported';
ALTER TABLE actor DROP CONSTRAINT actor_github_identity;
ALTER TABLE actor ADD CONSTRAINT actor_github_identity
    CHECK ((role = 'github') = (github_login IS NOT NULL)
           AND (role <> 'github' OR token_hash IS NULL)
           AND (token_hash IS NOT NULL OR role = 'github'
                OR (role = 'person' AND linear_id IS NOT NULL)
                OR (role = 'agent' AND kind = 'imported')));

-- What a finished job reports, such as an import's counts and notes.
ALTER TABLE job ADD COLUMN result jsonb;
ALTER TABLE job DROP CONSTRAINT job_kind_check;
ALTER TABLE job ADD CONSTRAINT job_kind_check
    CHECK (kind IN ('github_event', 'workflow_load', 'linear_import'));
