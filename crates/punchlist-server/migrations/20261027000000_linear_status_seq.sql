-- The event that last set an imported issue's status from Linear: its `created` event, or the
-- `transition` event of the import that moved it. A `transition` event after it moved the issue
-- in Punchlist, so a rerun keeps the issue's status.
ALTER TABLE issue ADD COLUMN linear_status_seq bigint;

-- An import's transitions commit with its job, so they carry the job's `finished_at` and actor.
UPDATE issue i SET linear_status_seq = (
    SELECT max(e.seq) FROM event e
    LEFT JOIN transition t ON t.id = e.transition_id
    WHERE e.issue_id = i.id
      AND (e.kind = 'created'
           OR (e.kind = 'transition' AND EXISTS (
                   SELECT 1 FROM job j
                   WHERE j.kind = 'linear_import' AND j.state = 'done'
                     AND j.finished_at = t.created_at
                     AND (j.payload ->> 'actor_id')::uuid = t.actor_id))))
WHERE i.linear_id IS NOT NULL;
