-- Slice 7: proof. A file an actor attached to an issue to show its work, such as a screenshot
-- from a live check. The bytes are on the server's disk under `storage_key`; `attempt` is the
-- issue's attempt when it was uploaded, how many times it had moved from Start to In Progress,
-- which `proof_attached` compares with the current one.
CREATE TABLE proof (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    issue_id     text NOT NULL REFERENCES issue (id),
    attempt      integer NOT NULL CHECK (attempt >= 0),
    kind         text NOT NULL CHECK (kind IN ('image', 'video', 'report')),
    title        text NOT NULL CHECK (title <> ''),
    content_type text NOT NULL,
    size         bigint NOT NULL CHECK (size >= 0),
    storage_key  text NOT NULL UNIQUE,
    actor_id     uuid NOT NULL REFERENCES actor (id),
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX proof_issue ON proof (issue_id, created_at);
