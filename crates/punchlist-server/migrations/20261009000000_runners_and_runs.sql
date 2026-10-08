-- Slice 2: runners, the runs they claim, each run's attempts and leases, and run logs.

-- A runner registers with a person's token and gets its own actor, whose role is `runner`,
-- so its transitions are recorded as the runner's.
CREATE TABLE runner (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id   uuid NOT NULL REFERENCES workspace (id),
    actor_id       uuid NOT NULL UNIQUE REFERENCES actor (id),
    name           text NOT NULL CHECK (name <> ''),
    -- Agent keys this runner can start, such as `claude-code`.
    agents         text[] NOT NULL,
    registered_by  uuid NOT NULL REFERENCES actor (id),
    registered_at  timestamptz NOT NULL DEFAULT now(),
    last_heartbeat timestamptz NOT NULL DEFAULT now()
);

-- One dispatch of an agent for an issue. A run has one or more attempts.
CREATE TABLE run (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id   uuid NOT NULL REFERENCES workspace (id),
    issue_id       text NOT NULL REFERENCES issue (id),
    -- The status whose dispatch rule started the run, such as `in_progress`.
    status         text NOT NULL,
    agent          text NOT NULL,
    branch         text NOT NULL,
    outcome        text NOT NULL DEFAULT 'running'
                   CHECK (outcome IN ('running', 'succeeded', 'failed')),
    started_at     timestamptz NOT NULL DEFAULT now(),
    finished_at    timestamptz,
    -- The last run_log.seq used.
    last_log_seq   bigint NOT NULL DEFAULT 0,
    CHECK ((outcome = 'running') = (finished_at IS NULL))
);

-- At most one running run per issue.
CREATE UNIQUE INDEX run_one_running_per_issue ON run (issue_id) WHERE outcome = 'running';
CREATE INDEX run_issue ON run (issue_id, started_at);

-- One try at a run by one runner, under a lease. The attempt number is the
-- compare-and-set token: a runner writes logs and outcomes only for the attempt it holds.
CREATE TABLE attempt (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id           uuid NOT NULL REFERENCES run (id),
    issue_id         text NOT NULL REFERENCES issue (id),
    runner_id        uuid NOT NULL REFERENCES runner (id),
    attempt          integer NOT NULL CHECK (attempt >= 1),
    state            text NOT NULL DEFAULT 'running'
                     CHECK (state IN ('running', 'succeeded', 'failed')),
    lease_expires_at timestamptz NOT NULL,
    failure_reason   text,
    started_at       timestamptz NOT NULL DEFAULT now(),
    finished_at      timestamptz,
    -- Reported by the runner when the attempt finishes.
    duration_ms      bigint CHECK (duration_ms >= 0),
    input_tokens     bigint CHECK (input_tokens >= 0),
    output_tokens    bigint CHECK (output_tokens >= 0),
    UNIQUE (run_id, attempt),
    CHECK ((state = 'running') = (finished_at IS NULL)),
    CHECK (state <> 'failed' OR failure_reason IS NOT NULL)
);

CREATE INDEX attempt_running_lease ON attempt (lease_expires_at) WHERE state = 'running';
CREATE INDEX attempt_runner ON attempt (runner_id) WHERE state = 'running';

-- Append-only. Lines are numbered per run in the order the server received them.
CREATE TABLE run_log (
    run_id     uuid NOT NULL REFERENCES run (id),
    seq        bigint NOT NULL,
    attempt    integer NOT NULL,
    line       text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, seq)
);
