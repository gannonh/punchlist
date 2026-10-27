-- PL-75: the token budget the dispatch rule named in the workflow version that queued the
-- run, so each of its attempts and resumes gets the same. NULL: no budget.
ALTER TABLE run ADD COLUMN max_tokens bigint;
-- The runner stopped the attempt's agent when its tokens went over the run's budget. Its run
-- is not dispatched again: the issue waits on a person.
ALTER TABLE attempt ADD COLUMN over_budget boolean NOT NULL DEFAULT false;
