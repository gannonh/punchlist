-- Slice 8: a runner restart neither loses nor duplicates a run. A run whose attempt was lost
-- gets another attempt of the same run, and a runner that restarts resumes the attempts it
-- still holds.

-- The agent session an attempt runs in: `claude --session-id` on a fresh attempt, and
-- `claude --resume` when the runner that holds it restarts.
ALTER TABLE attempt ADD COLUMN session_id uuid NOT NULL DEFAULT gen_random_uuid();

-- The workflow prompt the claim gave the run, so its later attempts and resumes get the same
-- one. Runs from before this migration have none.
ALTER TABLE run ADD COLUMN prompt text NOT NULL DEFAULT '';

-- At most one running attempt per issue, as at most one running run.
CREATE UNIQUE INDEX attempt_one_running_per_issue ON attempt (issue_id) WHERE state = 'running';
