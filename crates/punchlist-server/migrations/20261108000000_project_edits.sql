-- A renamed project or milestone tells the open app each of its issues' rows changed: a `project`
-- kind that, like `workflow`, the stream carries and a timeline leaves out. A `pass_when` edited
-- in Punchlist is kept by a later Linear import.
ALTER TABLE milestone ADD COLUMN pass_when_edited boolean NOT NULL DEFAULT false;
ALTER TABLE event DROP CONSTRAINT event_kind_check;
ALTER TABLE event ADD CONSTRAINT event_kind_check
    CHECK (kind IN ('transition', 'comment', 'refusal', 'created', 'edited',
                    'run', 'pull_request', 'proof', 'workflow', 'project'));
