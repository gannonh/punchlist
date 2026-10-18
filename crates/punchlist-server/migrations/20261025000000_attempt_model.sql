-- Slice 10: the issue page shows the model each attempt ran. A runner registers the model it
-- passes each agent, such as `{"claude-code": "haiku"}`, and a claim copies its agent's onto
-- the attempt. NULL when the runner set none and the agent used its own default.
ALTER TABLE runner ADD COLUMN models jsonb NOT NULL DEFAULT '{}';
ALTER TABLE attempt ADD COLUMN model text;
