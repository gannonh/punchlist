-- The open app updates a row's run and pull request state without a reload. Three event kinds
-- say only that an issue's row changed: `run`, `pull_request` and `proof`. The stream carries
-- them and an issue's timeline leaves them out, so no `pl` ever reads a kind it does not know.
ALTER TABLE event DROP CONSTRAINT event_kind_check;
ALTER TABLE event ADD CONSTRAINT event_kind_check
    CHECK (kind IN ('transition', 'comment', 'refusal', 'created', 'edited',
                    'run', 'pull_request', 'proof'));

-- An issue whose pull request evidence (the pull request, its check runs or its review threads)
-- was recorded and whose `pull_request` event is not written yet. Those writers do not take the
-- workspace's lock, which an event's sequence number needs, so each leaves a mark here in its
-- own transaction, and the server's sweep turns the marks into events under that lock. No
-- unique key and no foreign key, so writing a mark waits on no other transaction.
CREATE TABLE pull_request_change (
    id       bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    issue_id text NOT NULL
);
