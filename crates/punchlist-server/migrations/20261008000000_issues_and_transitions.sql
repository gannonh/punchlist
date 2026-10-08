-- Slice 1: workspaces, actors, issues, and the transitions and events that move them.

CREATE TABLE workspace (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name              text NOT NULL,
    -- Issue ids are the prefix and a number: PL-1.
    issue_prefix      text NOT NULL UNIQUE CHECK (issue_prefix ~ '^[A-Z][A-Z0-9]{0,9}$'),
    next_issue_number bigint NOT NULL DEFAULT 1,
    -- The last event sequence number used; events count up per workspace (ADR 0006).
    last_event_seq    bigint NOT NULL DEFAULT 0,
    created_at        timestamptz NOT NULL DEFAULT now()
);

-- One repository per workspace in v1 (ADR 0010), in its own table so a later phase can
-- allow several.
CREATE TABLE repository (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id   uuid NOT NULL UNIQUE REFERENCES workspace (id),
    owner          text NOT NULL,
    name           text NOT NULL,
    default_branch text NOT NULL DEFAULT 'main',
    created_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (owner, name)
);

CREATE TABLE actor (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspace (id),
    name         text NOT NULL,
    role         text NOT NULL CHECK (role IN ('person', 'agent', 'runner')),
    -- SHA-256 of the bearer token, in hex. The token itself is never stored.
    token_hash   text NOT NULL UNIQUE,
    created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE issue (
    -- The identifier people use, such as PL-1.
    id           text PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspace (id),
    number       bigint NOT NULL,
    title        text NOT NULL CHECK (title <> ''),
    body         text NOT NULL DEFAULT '',
    status       text NOT NULL,
    created_by   uuid NOT NULL REFERENCES actor (id),
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, number)
);

-- Append-only. The latest row for an issue matches issue.status.
CREATE TABLE transition (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    issue_id         text NOT NULL REFERENCES issue (id),
    from_status      text NOT NULL,
    to_status        text NOT NULL,
    actor_id         uuid NOT NULL REFERENCES actor (id),
    workflow_version text NOT NULL,
    created_at       timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX transition_issue ON transition (issue_id, created_at);

-- Append-only log behind each issue's timeline and, later, the event stream.
CREATE TABLE event (
    workspace_id  uuid NOT NULL REFERENCES workspace (id),
    seq           bigint NOT NULL,
    issue_id      text NOT NULL REFERENCES issue (id),
    kind          text NOT NULL CHECK (kind IN ('transition')),
    actor_id      uuid NOT NULL REFERENCES actor (id),
    transition_id uuid REFERENCES transition (id),
    created_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, seq),
    CHECK (kind <> 'transition' OR transition_id IS NOT NULL)
);

CREATE INDEX event_issue ON event (issue_id, seq);
