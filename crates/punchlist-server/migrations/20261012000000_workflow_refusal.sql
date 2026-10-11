-- The last workflow.toml the load job refused: its commit, when, and the reason logged.
-- Set by a refused load and cleared by the next load that activates a version.
ALTER TABLE workspace
    ADD COLUMN workflow_refused_commit text,
    ADD COLUMN workflow_refused_at     timestamptz,
    ADD COLUMN workflow_refused_reason text,
    ADD CONSTRAINT workflow_refusal_whole CHECK (
        (workflow_refused_commit IS NULL) = (workflow_refused_at IS NULL)
        AND (workflow_refused_commit IS NULL) = (workflow_refused_reason IS NULL)
    );
