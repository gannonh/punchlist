-- A person's Run again gives an issue whose runs in its status ran out another set, without a
-- transition: the next dispatch of the same entry starts a new round.

-- How many runs of the entry came before this dispatch's round: 0 for a transition's, the run
-- number it follows for a Run again's, and copied to each redispatch of the round. The limit
-- counts `run_number - runs_before`.
ALTER TABLE dispatch ADD COLUMN runs_before integer NOT NULL DEFAULT 0
    CHECK (runs_before >= 0 AND runs_before < run_number);

-- The workflow version a Run again's round runs under: the one active when the person asked,
-- copied to each redispatch of the round. NULL runs under the transition's version.
ALTER TABLE dispatch ADD COLUMN workflow_version text;

-- A Run again is on the issue's timeline, with the status and the agent it queued.
ALTER TABLE event ADD COLUMN status text, ADD COLUMN agent text;
ALTER TABLE event ADD CONSTRAINT event_run_again_has_status
    CHECK ((kind = 'run_again') = (status IS NOT NULL AND agent IS NOT NULL));
ALTER TABLE event DROP CONSTRAINT event_kind_check;
ALTER TABLE event ADD CONSTRAINT event_kind_check
    CHECK (kind IN ('transition', 'comment', 'refusal', 'created', 'edited',
                    'run', 'pull_request', 'proof', 'workflow', 'project', 'run_again'));
