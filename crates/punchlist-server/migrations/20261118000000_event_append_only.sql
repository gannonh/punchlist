-- The event log is append-only (PRD R6): the database refuses to rewrite or remove an event,
-- whoever asks. A migration that must change past events drops these triggers and says why.
CREATE FUNCTION event_append_only() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'event is append-only';
END
$$;

CREATE TRIGGER event_no_update_or_delete BEFORE UPDATE OR DELETE ON event
    FOR EACH ROW EXECUTE FUNCTION event_append_only();

CREATE TRIGGER event_no_truncate BEFORE TRUNCATE ON event
    FOR EACH STATEMENT EXECUTE FUNCTION event_append_only();
