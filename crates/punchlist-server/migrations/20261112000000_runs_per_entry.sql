-- How many runs the dispatch's round gets: the `runs_per_entry` of the workflow version it was
-- queued under (the transition's, or a Run again's), copied to each redispatch of the round,
-- so a push mid-entry does not change an entry already counting. Rows before it got 3.
ALTER TABLE dispatch ADD COLUMN runs_per_entry integer NOT NULL DEFAULT 3
    CHECK (runs_per_entry >= 1);
