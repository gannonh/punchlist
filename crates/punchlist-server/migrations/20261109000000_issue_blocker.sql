-- PL-109: an issue records the issues that block it. Both are issues of one workspace, and an
-- issue does not block itself. The server refuses a relation that would make a cycle, under the
-- workspace lock. An `edited` event's fields also name `blocked_by`.

CREATE TABLE issue_blocker (
    workspace_id uuid NOT NULL,
    issue_id     text NOT NULL,
    blocker_id   text NOT NULL,
    PRIMARY KEY (issue_id, blocker_id),
    FOREIGN KEY (workspace_id, issue_id) REFERENCES issue (workspace_id, id),
    FOREIGN KEY (workspace_id, blocker_id) REFERENCES issue (workspace_id, id),
    CONSTRAINT issue_not_its_own_blocker CHECK (issue_id <> blocker_id)
);

-- `blocks`, the reverse, reads by the blocker.
CREATE INDEX issue_blocker_blocker_id ON issue_blocker (blocker_id);
