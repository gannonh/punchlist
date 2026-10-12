-- The check suite a run belongs to: two workflows may each have a job of the same name, and a
-- rerun stays in its suite. Rows recorded before this column have none; they stay NULL (the
-- suite is not recoverable without asking GitHub, and they group by name alone, as before).
ALTER TABLE check_run ADD COLUMN check_suite_id bigint;
