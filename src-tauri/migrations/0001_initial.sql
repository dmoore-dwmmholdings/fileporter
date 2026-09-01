PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS settings (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  device_name TEXT NOT NULL DEFAULT '',
  receive_directory TEXT,
  onboarding_complete INTEGER NOT NULL DEFAULT 0 CHECK (onboarding_complete IN (0, 1)),
  receiving_enabled INTEGER NOT NULL DEFAULT 1 CHECK (receiving_enabled IN (0, 1)),
  launch_at_login INTEGER NOT NULL DEFAULT 1 CHECK (launch_at_login IN (0, 1)),
  notifications_enabled INTEGER NOT NULL DEFAULT 1 CHECK (notifications_enabled IN (0, 1)),
  revision INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT OR IGNORE INTO settings (singleton) VALUES (1);

CREATE TABLE IF NOT EXISTS schema_metadata (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_metadata (key, value) VALUES ('migration_version', '1');
