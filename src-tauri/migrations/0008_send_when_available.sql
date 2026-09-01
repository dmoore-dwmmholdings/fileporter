ALTER TABLE batches ADD COLUMN wait_for_available INTEGER NOT NULL DEFAULT 0 CHECK (wait_for_available IN (0, 1));
ALTER TABLE batch_targets ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0);
ALTER TABLE batch_targets ADD COLUMN wait_for_available INTEGER NOT NULL DEFAULT 0 CHECK (wait_for_available IN (0, 1));
