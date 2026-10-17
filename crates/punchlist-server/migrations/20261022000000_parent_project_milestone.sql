-- KAT-3776: issues carry a parent, a project and a milestone. A project belongs to a workspace,
-- and a milestone to a project, with the "PASS when" criteria that say it is done (PRD R17).
-- An `edited` event's fields also name `parent`, `project` and `milestone`.

CREATE TABLE project (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspace (id),
    name         text NOT NULL CHECK (name <> ''),
    created_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, name),
    UNIQUE (workspace_id, id)
);

CREATE TABLE milestone (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id uuid NOT NULL REFERENCES project (id),
    name       text NOT NULL CHECK (name <> ''),
    pass_when  text NOT NULL DEFAULT '',
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project_id, name),
    UNIQUE (project_id, id)
);

-- The composite keys keep the parent and the project in the issue's workspace, and the
-- milestone in the issue's project. The server refuses a parent that would make a cycle.
ALTER TABLE issue ADD CONSTRAINT issue_workspace_id_id_key UNIQUE (workspace_id, id);
ALTER TABLE issue
    ADD COLUMN parent_id text,
    ADD COLUMN project_id uuid,
    ADD COLUMN milestone_id uuid,
    ADD FOREIGN KEY (workspace_id, parent_id) REFERENCES issue (workspace_id, id),
    ADD FOREIGN KEY (workspace_id, project_id) REFERENCES project (workspace_id, id),
    ADD FOREIGN KEY (project_id, milestone_id) REFERENCES milestone (project_id, id),
    ADD CONSTRAINT issue_not_its_own_parent CHECK (parent_id <> id),
    ADD CONSTRAINT issue_milestone_in_project CHECK (milestone_id IS NULL OR project_id IS NOT NULL);
