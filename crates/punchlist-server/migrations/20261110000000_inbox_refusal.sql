-- A `refusal` inbox item is read apart from the other kinds for the same issue.
ALTER TABLE inbox_read DROP CONSTRAINT inbox_read_kind_check;
ALTER TABLE inbox_read ADD CONSTRAINT inbox_read_kind_check
    CHECK (kind IN ('waiting', 'run', 'refusal'));
