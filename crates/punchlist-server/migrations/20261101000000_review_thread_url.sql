-- The oldest recorded comment's URL on GitHub, stored only when it is a plain
-- `https://github.com/<owner>/<name>/pull/<number>#discussion_r<digits>`. Replaced with the
-- comments; NULL for a thread recorded before this, until it is next read with its comments.
ALTER TABLE review_thread ADD COLUMN url text;
