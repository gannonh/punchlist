-- Slice 9: issues carry an assignee and labels, and the event log records an issue's
-- creation and each edit of its fields, so the web app's stream sees them.

-- A person or an agent of the issue's workspace; the server checks both.
ALTER TABLE issue
    ADD COLUMN assignee_id uuid REFERENCES actor (id),
    ADD COLUMN labels text[] NOT NULL DEFAULT '{}';

ALTER TABLE event DROP CONSTRAINT event_kind_check;
ALTER TABLE event ADD CONSTRAINT event_kind_check
    CHECK (kind IN ('transition', 'comment', 'refusal', 'created', 'edited'));
-- The fields an edit changed, among `title`, `body`, `labels` and `assignee`.
ALTER TABLE event ADD COLUMN edited_fields text[];
ALTER TABLE event ADD CONSTRAINT event_edited_has_fields
    CHECK ((kind = 'edited') = (edited_fields IS NOT NULL));
