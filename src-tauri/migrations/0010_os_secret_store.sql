-- Private key material moves out of SQLite.  This table contains only a
-- migration marker; the platform credential store owns the secret bytes.
CREATE TABLE IF NOT EXISTS secret_store_migrations (
  name TEXT PRIMARY KEY,
  migrated_at INTEGER NOT NULL
);
