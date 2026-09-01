ALTER TABLE settings ADD COLUMN history_retention_days INTEGER NOT NULL DEFAULT 30 CHECK (history_retention_days IN (0, 7, 30, 90));
