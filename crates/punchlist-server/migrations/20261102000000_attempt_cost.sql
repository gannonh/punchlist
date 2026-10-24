-- PL-74: the cost in US dollars the agent reported at the end of the attempt (Claude Code's
-- `result` line), digit for digit. NULL when it reported none: Codex, or an attempt that
-- ended without a `result` line.
ALTER TABLE attempt ADD COLUMN cost_usd numeric CHECK (cost_usd >= 0);
