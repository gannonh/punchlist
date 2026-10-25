-- PL-65: the model and Codex effort the dispatch rule named in the workflow version that
-- queued the run, so each of its attempts and resumes gets the same. NULL: the runner's flag.
ALTER TABLE run ADD COLUMN model text, ADD COLUMN effort text;
