-- TLS private material is accessed only through the credential-store adapter;
-- it is never selected by snapshots or command DTOs.
CREATE TABLE IF NOT EXISTS local_tls_credentials (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  certificate_der BLOB NOT NULL,
  private_key_der BLOB NOT NULL,
  created_at INTEGER NOT NULL
);

-- Existing pairings have no discovered/manual endpoint. Keeping this nullable
-- preserves their trust record while requiring an endpoint before a send.
ALTER TABLE trusted_peers ADD COLUMN endpoint TEXT;
