-- Decisions may let a change that the person asked for run without the approval card.
ALTER TABLE decisions ADD COLUMN check_actions BOOLEAN NOT NULL DEFAULT TRUE;
