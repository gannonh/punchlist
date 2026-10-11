-- A transition that a GitHub event would have made and a gate refused. Append-only. The
-- issue stays where it is; this is what its timeline shows instead of the move.
CREATE TABLE refusal (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    issue_id         text NOT NULL REFERENCES issue (id),
    from_status      text NOT NULL,
    to_status        text NOT NULL,
    -- The first gate that failed, and why.
    gate             text NOT NULL,
    reason           text NOT NULL,
    actor_id         uuid NOT NULL REFERENCES actor (id),
    workflow_version text NOT NULL,
    -- The webhook delivery that brought the event; none when it was read from GitHub.
    delivery_id      text,
    created_at       timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX refusal_issue ON refusal (issue_id, created_at);

ALTER TABLE event DROP CONSTRAINT event_kind_check;
ALTER TABLE event ADD CONSTRAINT event_kind_check
    CHECK (kind IN ('transition', 'comment', 'refusal'));
ALTER TABLE event ADD COLUMN refusal_id uuid REFERENCES refusal (id);
ALTER TABLE event ADD CONSTRAINT event_refusal_has_refusal
    CHECK (kind <> 'refusal' OR refusal_id IS NOT NULL);
