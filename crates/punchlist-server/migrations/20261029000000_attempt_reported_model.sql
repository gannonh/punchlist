-- PL-60: the model the agent said it ran on, read from its log lines as they arrive (Claude
-- Code's `init` line or first assistant message, Codex's `thread/started`). NULL until it
-- reports one. `model` stays what the runner pinned.
ALTER TABLE attempt ADD COLUMN reported_model text;
