-- The GitHub Actions job behind a check run, as a gated request's read of GitHub found it:
-- `ci_green` counts a required `github-actions` check only when a runner took its job and the
-- job's workflow file at the head commit is the default branch's. `workflow_path` is that
-- file, set only for a job with a runner; `workflow_sha` is its blob at the head commit and
-- `workflow_default_sha` the same path's blob on the default branch at the read. A delivery
-- carries none of this and leaves the columns as they are; rows from before them stay NULL
-- until a gated request reads the commit again, and satisfy no required GitHub Actions check.
ALTER TABLE check_run
    ADD COLUMN workflow_path text,
    ADD COLUMN workflow_sha text,
    ADD COLUMN workflow_default_sha text;
