-- `GET /api/search` (ADR 0011): trigram indexes answer `ILIKE '%text%'` over the issue's id,
-- title and body and over comments. pg_trgm is a trusted extension, so the migration user
-- needs only CREATE on the database.
CREATE EXTENSION IF NOT EXISTS pg_trgm;

CREATE INDEX issue_id_trgm ON issue USING gin (id gin_trgm_ops);
CREATE INDEX issue_title_trgm ON issue USING gin (title gin_trgm_ops);
CREATE INDEX issue_body_trgm ON issue USING gin (body gin_trgm_ops);
CREATE INDEX comment_body_trgm ON comment USING gin (body gin_trgm_ops);
