CREATE TABLE IF NOT EXISTS trusted_peers (
  device_id TEXT PRIMARY KEY,
  public_key BLOB NOT NULL,
  certificate_fingerprint TEXT NOT NULL,
  remote_name TEXT NOT NULL,
  local_alias TEXT,
  paired_at INTEGER NOT NULL,
  last_seen_at INTEGER,
  auto_send INTEGER NOT NULL DEFAULT 0 CHECK (auto_send IN (0, 1)),
  revoked_at INTEGER
);

CREATE TABLE IF NOT EXISTS batches (
  id TEXT PRIMARY KEY,
  direction TEXT NOT NULL,
  state TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  completed_at INTEGER,
  total_bytes INTEGER NOT NULL CHECK (total_bytes >= 0),
  total_entries INTEGER NOT NULL CHECK (total_entries >= 0),
  warning_count INTEGER NOT NULL DEFAULT 0 CHECK (warning_count >= 0)
);

CREATE TABLE IF NOT EXISTS batch_targets (
  id TEXT PRIMARY KEY,
  batch_id TEXT NOT NULL REFERENCES batches(id) ON DELETE CASCADE,
  peer_device_id TEXT NOT NULL REFERENCES trusted_peers(device_id) ON DELETE RESTRICT,
  state TEXT NOT NULL,
  acknowledged_bytes INTEGER NOT NULL DEFAULT 0 CHECK (acknowledged_bytes >= 0),
  error_code TEXT,
  retry_at INTEGER
);

CREATE TABLE IF NOT EXISTS items (
  id TEXT PRIMARY KEY,
  batch_id TEXT NOT NULL REFERENCES batches(id) ON DELETE CASCADE,
  parent_item_id TEXT REFERENCES items(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  display_name TEXT NOT NULL,
  source_path_local TEXT,
  destination_path_local TEXT,
  size INTEGER NOT NULL CHECK (size >= 0),
  mtime INTEGER,
  state TEXT NOT NULL,
  warning_json TEXT
);

CREATE TABLE IF NOT EXISTS checkpoints (
  target_id TEXT NOT NULL REFERENCES batch_targets(id) ON DELETE CASCADE,
  item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  durable_offset INTEGER NOT NULL CHECK (durable_offset >= 0),
  verified_hash BLOB,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (target_id, item_id)
);

CREATE INDEX IF NOT EXISTS batch_targets_batch_id_idx ON batch_targets(batch_id);
CREATE INDEX IF NOT EXISTS batch_targets_peer_device_id_idx ON batch_targets(peer_device_id);
CREATE INDEX IF NOT EXISTS items_batch_id_idx ON items(batch_id);
