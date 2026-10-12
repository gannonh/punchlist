-- The GitHub App that created a check run, by slug (`github-actions`): `ci_green` requires a
-- named check from the app that owns it. Rows recorded before this column have none; they stay
-- NULL until a gated request reads the commit's runs from GitHub again, and a run with no app
-- satisfies no required check.
ALTER TABLE check_run ADD COLUMN app text;
