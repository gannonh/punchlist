-- The branches other than the default branch that a pull request from the run's head branch
-- has been into, open or closed, earlier bases too, as a gated request's read of GitHub found
-- them for a check run whose job it found. A pull request's run takes its workflow file from
-- the head commit merged into the base, and GitHub does not say which pull request a run was
-- for, so `ci_green` counts a required `github-actions` check only when this is empty. NULL
-- when no pull request from a branch of the workspace's repository is behind the run (a fork's
-- run, a branch with none), when no job was found, and for rows from before the column: none
-- of them satisfies a required GitHub Actions check. A delivery leaves it as it is.
ALTER TABLE check_run ADD COLUMN workflow_other_bases text[];
