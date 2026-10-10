-- Slice 4: workflow versions loaded from the repository, gate results on transitions, and
-- agent actors that act for one run.

-- A workflow loaded from `.punchlist/` on the repository's default branch. Only valid
-- versions are stored; a refused one leaves the active version in place (ADR 0001).
CREATE TABLE workflow_version (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id  uuid NOT NULL REFERENCES workspace (id),
    -- The content hash over workflow.toml and its prompts, recorded on transitions.
    hash          text NOT NULL,
    -- The default branch commit it was first loaded from.
    commit_sha    text NOT NULL,
    workflow_toml text NOT NULL,
    -- Prompt files by their path under .punchlist/.
    prompts       jsonb NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, hash)
);

-- NULL until a version loads: the workspace uses the built-in workflow (ADR 0010).
ALTER TABLE workspace ADD COLUMN workflow_version_id uuid REFERENCES workflow_version (id);

-- Each gate's result on a transition, in the workflow's order.
CREATE TABLE transition_gate (
    transition_id uuid NOT NULL REFERENCES transition (id),
    position      integer NOT NULL,
    gate          text NOT NULL,
    result        text NOT NULL CHECK (result IN ('pass', 'fail')),
    reason        text NOT NULL,
    PRIMARY KEY (transition_id, position)
);

-- An agent actor acts for one run, and its token works only while the run is running.
ALTER TABLE actor ADD COLUMN run_id uuid REFERENCES run (id);
ALTER TABLE actor ADD CONSTRAINT actor_agent_run CHECK ((role = 'agent') = (run_id IS NOT NULL));

ALTER TABLE job DROP CONSTRAINT job_kind_check;
ALTER TABLE job ADD CONSTRAINT job_kind_check CHECK (kind IN ('github_event', 'workflow_load'));
