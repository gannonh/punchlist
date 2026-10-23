-- What the run's agent said a person must do (`POST /api/runs/{id}/waiting`). A run with one
-- is not dispatched again when it ends.
ALTER TABLE run ADD COLUMN waiting_reason text;
