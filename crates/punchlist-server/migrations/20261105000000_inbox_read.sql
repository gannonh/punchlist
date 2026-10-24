-- The newest inbox item each person has opened, per issue and kind. An item is unread while
-- its `since_seq`, the event that made it, is above the one recorded here.
CREATE TABLE inbox_read (
    actor_id  uuid NOT NULL REFERENCES actor (id),
    issue_id  text NOT NULL REFERENCES issue (id),
    kind      text NOT NULL CHECK (kind IN ('waiting')),
    since_seq bigint NOT NULL,
    PRIMARY KEY (actor_id, issue_id, kind)
);
