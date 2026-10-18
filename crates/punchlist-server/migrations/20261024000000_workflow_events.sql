-- A load of a different workflow version tells the open app every row changed: a fourth kind
-- that, like `run`, `pull_request` and `proof`, the stream carries and a timeline leaves out.
ALTER TABLE event DROP CONSTRAINT event_kind_check;
ALTER TABLE event ADD CONSTRAINT event_kind_check
    CHECK (kind IN ('transition', 'comment', 'refusal', 'created', 'edited',
                    'run', 'pull_request', 'proof', 'workflow'));
