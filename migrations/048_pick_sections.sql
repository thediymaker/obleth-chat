-- Decisions pick the sections of a paper that answer what research is looking for.
ALTER TABLE decisions ADD COLUMN pick_sections BOOLEAN NOT NULL DEFAULT TRUE;
