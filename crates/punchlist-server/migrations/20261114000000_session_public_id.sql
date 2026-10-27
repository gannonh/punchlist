-- The id a person sees and revokes a session by (PL-168). The primary key is the hash of the
-- cookie's value, so it is never sent back.
ALTER TABLE session ADD COLUMN public_id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid();
