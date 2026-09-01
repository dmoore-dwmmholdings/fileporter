ALTER TABLE settings
ADD COLUMN automatic_device_trust INTEGER NOT NULL DEFAULT 1
CHECK (automatic_device_trust IN (0, 1));
