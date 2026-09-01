CREATE TABLE IF NOT EXISTS local_identity (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  secret_key BLOB NOT NULL CHECK (length(secret_key) = 32),
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS pending_pairings (
  id TEXT PRIMARY KEY,
  device_id TEXT NOT NULL,
  public_key BLOB NOT NULL CHECK (length(public_key) = 32),
  certificate_fingerprint TEXT NOT NULL,
  remote_name TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  local_confirmed INTEGER NOT NULL DEFAULT 0 CHECK (local_confirmed IN (0, 1))
);

CREATE INDEX IF NOT EXISTS pending_pairings_expires_at_idx ON pending_pairings(expires_at);
