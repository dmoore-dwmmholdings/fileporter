CREATE TABLE IF NOT EXISTS incoming_notification_ledger (
    batch_id TEXT PRIMARY KEY REFERENCES batches(id) ON DELETE CASCADE,
    notified_at INTEGER NOT NULL
);
