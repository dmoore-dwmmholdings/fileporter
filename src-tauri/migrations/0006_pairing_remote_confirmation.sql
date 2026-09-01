ALTER TABLE pending_pairings ADD COLUMN remote_confirmed INTEGER NOT NULL DEFAULT 0 CHECK (remote_confirmed IN (0, 1));
