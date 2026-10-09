-- Slice 3: GitHub webhook deliveries as jobs, the pull requests, check runs and review
-- threads they record as evidence, GitHub actors, and comments on issues.

-- A GitHub actor is one GitHub user, by login, acting through a webhook event. It has no
-- bearer token: it never calls the API.
ALTER TABLE actor DROP CONSTRAINT actor_role_check;
ALTER TABLE actor ADD CONSTRAINT actor_role_check
    CHECK (role IN ('person', 'agent', 'runner', 'github'));
ALTER TABLE actor ALTER COLUMN token_hash DROP NOT NULL;
ALTER TABLE actor ADD COLUMN github_login text;
ALTER TABLE actor ADD CONSTRAINT actor_github_identity
    CHECK ((role = 'github') = (github_login IS NOT NULL)
           AND (role = 'github') = (token_hash IS NULL));
CREATE UNIQUE INDEX actor_github_login ON actor (workspace_id, github_login)
    WHERE github_login IS NOT NULL;

-- The webhook delivery that caused a transition, for transitions made on a GitHub event.
ALTER TABLE transition ADD COLUMN delivery_id text;

-- Background jobs (ADR 0007). Claimed with FOR UPDATE SKIP LOCKED under a lease; a job's
-- writes and its completion commit together. The idempotency key, such as a webhook
-- delivery id, makes enqueueing the same work twice a no-op.
CREATE TABLE job (
    id               bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind             text NOT NULL CHECK (kind IN ('github_event')),
    idempotency_key  text NOT NULL UNIQUE,
    payload          jsonb NOT NULL,
    state            text NOT NULL DEFAULT 'queued'
                     CHECK (state IN ('queued', 'running', 'done', 'failed')),
    attempts         integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    run_after        timestamptz NOT NULL DEFAULT now(),
    lease_expires_at timestamptz,
    last_error       text,
    created_at       timestamptz NOT NULL DEFAULT now(),
    finished_at      timestamptz,
    CHECK ((state = 'running') = (lease_expires_at IS NOT NULL)),
    CHECK ((state IN ('done', 'failed')) = (finished_at IS NOT NULL))
);

CREATE INDEX job_ready ON job (run_after, id) WHERE state = 'queued';
CREATE INDEX job_running_lease ON job (lease_expires_at) WHERE state = 'running';

-- A pull request on the workspace's repository, linked or not.
CREATE TABLE pull_request (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    repository_id   uuid NOT NULL REFERENCES repository (id),
    number          bigint NOT NULL CHECK (number > 0),
    -- GitHub's id for the pull request.
    github_id       bigint NOT NULL,
    title           text NOT NULL,
    branch          text NOT NULL,
    head_sha        text NOT NULL,
    url             text NOT NULL,
    author_login    text NOT NULL,
    state           text NOT NULL CHECK (state IN ('open', 'closed', 'merged')),
    draft           boolean NOT NULL,
    -- GitHub's mergeable_state, such as `clean` or `dirty`; `unknown` until GitHub computes it.
    merge_state     text NOT NULL DEFAULT 'unknown',
    -- Over the check runs on head_sha: `none`, `pending`, `passing` or `failing`.
    checks          text NOT NULL DEFAULT 'none'
                    CHECK (checks IN ('none', 'pending', 'passing', 'failing')),
    checks_total    integer NOT NULL DEFAULT 0,
    checks_passed   integer NOT NULL DEFAULT 0,
    open_threads    integer NOT NULL DEFAULT 0 CHECK (open_threads >= 0),
    -- The issue its branch or title names; NULL for an unlinked pull request.
    issue_id        text REFERENCES issue (id),
    -- GitHub's updated_at for the last pull_request payload applied, so an older delivery
    -- arriving late does not overwrite a newer one.
    github_updated_at timestamptz NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (repository_id, number)
);

CREATE INDEX pull_request_issue ON pull_request (issue_id) WHERE issue_id IS NOT NULL;
CREATE INDEX pull_request_head ON pull_request (repository_id, head_sha);

-- Check runs belong to a commit; a pull request's checks are the runs on its head commit.
CREATE TABLE check_run (
    -- GitHub's id for the check run.
    id            bigint PRIMARY KEY,
    repository_id uuid NOT NULL REFERENCES repository (id),
    head_sha      text NOT NULL,
    name          text NOT NULL,
    status        text NOT NULL,
    -- Set when status is `completed`, such as `success` or `failure`.
    conclusion    text,
    updated_at    timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX check_run_head ON check_run (repository_id, head_sha);

-- Review threads, from pull_request_review_thread events.
CREATE TABLE review_thread (
    -- GitHub's node id for the thread.
    node_id         text PRIMARY KEY,
    pull_request_id uuid NOT NULL REFERENCES pull_request (id),
    resolved        boolean NOT NULL,
    updated_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX review_thread_pull_request ON review_thread (pull_request_id);

CREATE TABLE comment (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    issue_id     text NOT NULL REFERENCES issue (id),
    actor_id     uuid NOT NULL REFERENCES actor (id),
    body         text NOT NULL CHECK (body <> ''),
    -- The transition this comment was required for, if any.
    transition_id uuid REFERENCES transition (id),
    created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX comment_issue ON comment (issue_id, created_at);

ALTER TABLE event DROP CONSTRAINT event_kind_check;
ALTER TABLE event ADD CONSTRAINT event_kind_check CHECK (kind IN ('transition', 'comment'));
ALTER TABLE event ADD COLUMN comment_id uuid REFERENCES comment (id);
ALTER TABLE event ADD CONSTRAINT event_comment_has_comment
    CHECK (kind <> 'comment' OR comment_id IS NOT NULL);
