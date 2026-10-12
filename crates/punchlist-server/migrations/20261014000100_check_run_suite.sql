-- The check suite a run belongs to: two workflows may each have a job of the same name, and a
-- rerun stays in its suite. Rows recorded before this column have none; they stay NULL (the
-- suite is not recoverable without asking GitHub). A NULL-suite row counts only until a newer
-- run of its name is recorded on the commit (`newest_check_runs`).
ALTER TABLE check_run ADD COLUMN check_suite_id bigint;
