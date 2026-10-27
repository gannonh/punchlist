-- PL-185: the dispatch rule's agent this run stood in for, such as `codex`, when that agent
-- was paused for its usage limit and the rule's `fallback` ran instead. NULL: the rule's own.
ALTER TABLE run ADD COLUMN fallback_for text;
