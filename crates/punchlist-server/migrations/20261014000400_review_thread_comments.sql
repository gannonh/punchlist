-- A review thread's newest comments, oldest first: a JSON array of
-- {"author": <login>, "body": <text>, "cut": <the comment was longer>}, at most the last 10,
-- each body at most 2000 characters. `comments_total` is how many the thread has. Replaced
-- each time the thread is read. NULL for a thread recorded before this, until it is next read.
ALTER TABLE review_thread ADD COLUMN comments jsonb, ADD COLUMN comments_total integer;
