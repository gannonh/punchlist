-- A run that ends with its issue still in the status that dispatched it queues one more
-- dispatch for the same transition, so a transition has one dispatch per run it gets.

-- The dispatch's place among those of its transition: 1 is the one the transition queued,
-- and the end of run n queues n + 1, up to the server's limit. The highest number is how
-- many runs this entry into the status has been given.
ALTER TABLE dispatch ADD COLUMN run_number integer NOT NULL DEFAULT 1 CHECK (run_number >= 1);

-- Was unique on the transition alone. A transition still queues one dispatch, and the end
-- of a run queues at most one: the second writer of a number is refused.
ALTER TABLE dispatch DROP CONSTRAINT dispatch_transition_id_key;
ALTER TABLE dispatch ADD UNIQUE (transition_id, run_number);
