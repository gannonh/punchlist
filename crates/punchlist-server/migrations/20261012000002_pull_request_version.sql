-- Orders reads of a pull request from GitHub. A reader notes the version before it reads and
-- writes what it read only if the version is still that, adding one; a payload recorded
-- without a read leaves it alone. 0 for a pull request no read has been recorded on.
ALTER TABLE pull_request ADD COLUMN version bigint NOT NULL DEFAULT 0;
