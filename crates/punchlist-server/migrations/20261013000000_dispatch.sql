-- Slice 5: an issue that enters a status with a dispatch rule waits here for a runner, and
-- review threads carry the place they are on.

-- One row per transition into a status with a `dispatch.status` rule. It waits (`run_id`
-- NULL) until a runner that can run `agent` claims it, which sets `run_id` in the claim's
-- transaction. The issue's next transition deletes a row still waiting, so an issue has at
-- most one, and it is for the status the issue is in.
CREATE TABLE dispatch (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    issue_id      text NOT NULL REFERENCES issue (id),
    -- The transition that put the issue in the status, whose `to_status` it is for. Unique:
    -- a transition queues one dispatch.
    transition_id uuid NOT NULL UNIQUE REFERENCES transition (id),
    -- The agent key of that status's dispatch rule when the issue entered it.
    agent         text NOT NULL,
    -- The run a runner claimed it with.
    run_id        uuid UNIQUE REFERENCES run (id),
    created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX dispatch_waiting ON dispatch (issue_id) WHERE run_id IS NULL;

-- Where a review thread is: its file, and its line, or the line it was on before the file
-- changed under it. NULL for a thread recorded before this, until it is next read.
ALTER TABLE review_thread ADD COLUMN path text, ADD COLUMN line bigint;
