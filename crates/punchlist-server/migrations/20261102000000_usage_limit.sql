-- A run that ended on its agent's usage limit gives its dispatch back to the issue, waiting
-- until the limit resets: no runner claims a dispatch before its `not_before`.
ALTER TABLE dispatch ADD COLUMN not_before timestamptz;

-- The agent kinds a runner claims nothing for until their usage limit resets: the limit is
-- the account's the runner runs the agent as.
CREATE TABLE runner_pause (
    runner_id uuid NOT NULL REFERENCES runner (id),
    agent     text NOT NULL,
    until     timestamptz NOT NULL,
    PRIMARY KEY (runner_id, agent)
);
