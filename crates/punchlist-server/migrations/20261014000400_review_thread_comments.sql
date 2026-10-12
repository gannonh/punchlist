-- A review thread's comments as GitHub lists them, oldest first: a JSON array of
-- {"author": <login>, "body": <text>}, at most the first 10, each body cut at 2000
-- characters. `comments_total` is how many the thread has. Replaced each time the thread is
-- read. NULL for a thread recorded before this, until it is next read.
ALTER TABLE review_thread ADD COLUMN comments jsonb, ADD COLUMN comments_total integer;
