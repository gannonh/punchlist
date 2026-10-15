-- Slice 6: agents have their own identities. An agent actor names its agent kind and the
-- person who owns it, and a person can mint a standalone agent with no run.

-- `kind` is the agent key, such as `codex`; `model` is the model it runs, when known;
-- `owner_id` is the person it acts for. A run's agent is owned by the person who registered
-- the runner that claimed the run.
ALTER TABLE actor
    ADD COLUMN kind text,
    ADD COLUMN model text,
    ADD COLUMN owner_id uuid REFERENCES actor (id);

UPDATE actor a SET kind = r.agent, owner_id = rn.registered_by
FROM run r
JOIN attempt t ON t.run_id = r.id AND t.attempt = 1
JOIN runner rn ON rn.id = t.runner_id
WHERE a.run_id = r.id;

-- An agent has a kind and an owner, and a run only when it acts for one: a standalone agent's
-- token has no run. Nobody else has any of these.
ALTER TABLE actor DROP CONSTRAINT actor_agent_run;
ALTER TABLE actor ADD CONSTRAINT actor_agent_identity CHECK (
    CASE WHEN role = 'agent' THEN kind IS NOT NULL AND kind <> '' AND owner_id IS NOT NULL
         ELSE kind IS NULL AND model IS NULL AND owner_id IS NULL AND run_id IS NULL
    END
);
