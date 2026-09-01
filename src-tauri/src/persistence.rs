use crate::error::AppError;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const INITIAL_MIGRATION: &str = include_str!("../migrations/0001_initial.sql");
const TRANSFER_HISTORY_MIGRATION: &str = include_str!("../migrations/0002_transfer_history.sql");
const IDENTITY_PAIRING_MIGRATION: &str = include_str!("../migrations/0003_identity_pairing.sql");
const LISTENER_ADDRESS_MIGRATION: &str = include_str!("../migrations/0004_listener_address.sql");
const TLS_CREDENTIALS_AND_ENDPOINTS_MIGRATION: &str =
    include_str!("../migrations/0005_tls_credentials_and_peer_endpoints.sql");
const PAIRING_REMOTE_CONFIRMATION_MIGRATION: &str =
    include_str!("../migrations/0006_pairing_remote_confirmation.sql");
const PAIRING_SAS_MIGRATION: &str = include_str!("../migrations/0007_pairing_sas.sql");
const SEND_WHEN_AVAILABLE_MIGRATION: &str =
    include_str!("../migrations/0008_send_when_available.sql");
const NOTIFICATION_LEDGER_MIGRATION: &str =
    include_str!("../migrations/0009_notification_ledger.sql");
const OS_SECRET_STORE_MIGRATION: &str = include_str!("../migrations/0010_os_secret_store.sql");
const SETTINGS_CONTRACT_MIGRATION: &str = include_str!("../migrations/0011_settings_contract.sql");
const AUTOMATIC_DEVICE_TRUST_MIGRATION: &str =
    include_str!("../migrations/0012_automatic_device_trust.sql");
const LAN_LISTENER_DEFAULT_MIGRATION: &str =
    include_str!("../migrations/0013_lan_listener_default.sql");

const BUSY_TIMEOUT_MS: u64 = 5_000;
pub(crate) type LegacyTlsCredentials = (Vec<u8>, Vec<u8>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedPeer {
    pub device_id: String,
    pub public_key: Vec<u8>,
    pub certificate_fingerprint: String,
    pub remote_name: String,
    pub local_alias: Option<String>,
    pub paired_at: i64,
    pub last_seen_at: Option<i64>,
    pub auto_send: bool,
    pub revoked_at: Option<i64>,
    pub endpoint: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPairing {
    pub id: String,
    pub device_id: String,
    pub public_key: Vec<u8>,
    pub certificate_fingerprint: String,
    pub remote_name: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub local_confirmed: bool,
    pub remote_confirmed: bool,
    /// A normalized, proof-derived comparison code. It contains no transcript,
    /// signature, certificate, or private identity material.
    pub sas_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub id: String,
    pub direction: String,
    pub state: String,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    pub total_bytes: i64,
    pub total_entries: i64,
    pub warning_count: i64,
    pub wait_for_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchTarget {
    pub id: String,
    pub batch_id: String,
    pub peer_device_id: String,
    pub state: String,
    pub acknowledged_bytes: i64,
    pub error_code: Option<String>,
    pub retry_at: Option<i64>,
    pub retry_count: i64,
    pub wait_for_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferItem {
    pub id: String,
    pub batch_id: String,
    pub parent_item_id: Option<String>,
    pub kind: String,
    pub display_name: String,
    pub source_path_local: Option<String>,
    pub destination_path_local: Option<String>,
    pub size: i64,
    pub mtime: Option<i64>,
    pub state: String,
    pub warning_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub target_id: String,
    pub item_id: String,
    pub durable_offset: i64,
    pub verified_hash: Option<Vec<u8>>,
    pub updated_at: i64,
}

/// A complete persisted batch snapshot.  This is deliberately transport-agnostic: a
/// queued record means it has been accepted locally, never that a peer received it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedBatch {
    pub batch: Batch,
    pub targets: Vec<BatchTarget>,
    pub items: Vec<TransferItem>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub device_name: String,
    pub receive_directory: Option<String>,
    pub onboarding_complete: bool,
    pub receiving_enabled: bool,
    pub listen_address: String,
    pub launch_at_login: bool,
    pub notifications_enabled: bool,
    /// Trust authenticated Fileporter identities discovered on the local network
    /// without requiring a human comparison-code confirmation.
    pub automatic_device_trust: bool,
    /// 0 means keep history forever; positive values are the documented days.
    pub history_retention_days: i64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            device_name: String::new(),
            receive_directory: None,
            onboarding_complete: false,
            receiving_enabled: true,
            listen_address: "0.0.0.0:0".into(),
            launch_at_login: true,
            notifications_enabled: true,
            automatic_device_trust: true,
            history_retention_days: 30,
        }
    }
}
pub struct SettingsRepository {
    connection: Mutex<Connection>,
    #[cfg(test)]
    fail_checkpoint_writes: std::sync::atomic::AtomicBool,
}
impl SettingsRepository {
    pub fn open(database_path: PathBuf) -> Result<Self, AppError> {
        if let Some(parent) = database_path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| AppError::DataDirectoryUnavailable)?;
        }
        let mut connection = Connection::open(&database_path).map_err(AppError::Database)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(AppError::Database)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(AppError::Database)?;
        connection
            .busy_timeout(std::time::Duration::from_millis(BUSY_TIMEOUT_MS))
            .map_err(AppError::Database)?;
        apply_migrations(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
            #[cfg(test)]
            fail_checkpoint_writes: std::sync::atomic::AtomicBool::new(false),
        })
    }
    pub fn load(&self) -> Result<Settings, AppError> {
        let connection = self
            .connection
            .lock()
            .expect("settings repository mutex poisoned");
        connection.query_row("SELECT device_name, receive_directory, onboarding_complete, receiving_enabled, listen_address, launch_at_login, notifications_enabled, history_retention_days, automatic_device_trust FROM settings WHERE singleton = 1", [], |row| Ok(Settings { device_name: row.get(0)?, receive_directory: row.get(1)?, onboarding_complete: row.get::<_, i64>(2)? != 0, receiving_enabled: row.get::<_, i64>(3)? != 0, listen_address: row.get(4)?, launch_at_login: row.get::<_, i64>(5)? != 0, notifications_enabled: row.get::<_, i64>(6)? != 0, history_retention_days: row.get(7)?, automatic_device_trust: row.get::<_, i64>(8)? != 0 })).optional().map_err(AppError::Database)?.ok_or_else(|| AppError::Database(rusqlite::Error::QueryReturnedNoRows))
    }
    pub fn save(&self, settings: &Settings) -> Result<(), AppError> {
        let connection = self
            .connection
            .lock()
            .expect("settings repository mutex poisoned");
        connection.execute("UPDATE settings SET device_name = ?1, receive_directory = ?2, onboarding_complete = ?3, receiving_enabled = ?4, listen_address = ?5, launch_at_login = ?6, notifications_enabled = ?7, history_retention_days = ?8, automatic_device_trust = ?9, revision = revision + 1, updated_at = CURRENT_TIMESTAMP WHERE singleton = 1", rusqlite::params![settings.device_name, settings.receive_directory, settings.onboarding_complete as i64, settings.receiving_enabled as i64, settings.listen_address, settings.launch_at_login as i64, settings.notifications_enabled as i64, settings.history_retention_days, settings.automatic_device_trust as i64]).map_err(AppError::Database)?;
        Ok(())
    }

    /// The applied schema value is operational metadata, not user content.
    pub fn migration_version(&self) -> Result<i64, AppError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT value FROM schema_metadata WHERE key = 'migration_version'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .and_then(|value| {
                    value
                        .parse::<i64>()
                        .map_err(|_| rusqlite::Error::InvalidQuery)
                })
        })
    }

    pub fn upsert_trusted_peer(&self, peer: &TrustedPeer) -> Result<(), AppError> {
        if let Some(endpoint) = peer.endpoint.as_deref() {
            crate::engine::validate_manual_endpoint(endpoint).map_err(|_| {
                AppError::Validation {
                    code: "invalid_endpoint",
                    message: "The trusted device endpoint must be loopback or private-network.",
                    field: Some("endpoint"),
                }
            })?;
        }
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO trusted_peers (device_id, public_key, certificate_fingerprint, remote_name, local_alias, paired_at, last_seen_at, auto_send, revoked_at, endpoint) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(device_id) DO UPDATE SET public_key = excluded.public_key, certificate_fingerprint = excluded.certificate_fingerprint, remote_name = excluded.remote_name, local_alias = excluded.local_alias, paired_at = excluded.paired_at, last_seen_at = excluded.last_seen_at, auto_send = excluded.auto_send, revoked_at = excluded.revoked_at, endpoint = excluded.endpoint",
                params![peer.device_id, peer.public_key, peer.certificate_fingerprint, peer.remote_name, peer.local_alias, peer.paired_at, peer.last_seen_at, peer.auto_send as i64, peer.revoked_at, peer.endpoint],
            )?;
            Ok(())
        })
    }

    pub fn trusted_peer(&self, device_id: &str) -> Result<Option<TrustedPeer>, AppError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT device_id, public_key, certificate_fingerprint, remote_name, local_alias, paired_at, last_seen_at, auto_send, revoked_at, endpoint FROM trusted_peers WHERE device_id = ?1",
                    [device_id],
                    trusted_peer_from_row,
                )
                .optional()
        })
    }
    pub fn active_trusted_peers(&self) -> Result<Vec<TrustedPeer>, AppError> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT device_id, public_key, certificate_fingerprint, remote_name, local_alias, paired_at, last_seen_at, auto_send, revoked_at, endpoint FROM trusted_peers WHERE revoked_at IS NULL ORDER BY paired_at DESC")?;
            let peers = statement.query_map([], trusted_peer_from_row)?.collect();
            peers
        })
    }
    pub fn revoke_trusted_peer(&self, device_id: &str, revoked_at: i64) -> Result<(), AppError> {
        let connection = self
            .connection
            .lock()
            .expect("settings repository mutex poisoned");
        let changed = connection.execute("UPDATE trusted_peers SET revoked_at = ?2 WHERE device_id = ?1 AND revoked_at IS NULL", params![device_id, revoked_at]).map_err(AppError::Database)?;
        if changed == 0 {
            return Err(AppError::Validation {
                code: "device_not_found",
                message: "That trusted device is no longer available.",
                field: Some("deviceId"),
            });
        }
        Ok(())
    }
    pub(crate) fn legacy_identity_secret(&self) -> Result<Option<Vec<u8>>, AppError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT secret_key FROM local_identity WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
        })
    }
    #[cfg(test)]
    pub(crate) fn save_legacy_identity_secret_for_test(
        &self,
        secret: &[u8; 32],
        created_at: i64,
    ) -> Result<(), AppError> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO local_identity (singleton, secret_key, created_at) VALUES (1, ?1, ?2)",
                params![secret.as_slice(), created_at],
            )?;
            Ok(())
        })
    }
    pub(crate) fn legacy_tls_credentials(&self) -> Result<Option<LegacyTlsCredentials>, AppError> {
        self.with_connection(|connection| connection.query_row(
            "SELECT certificate_der, private_key_der FROM local_tls_credentials WHERE singleton = 1", [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional())
    }
    #[cfg(test)]
    pub(crate) fn save_legacy_tls_credentials_for_test(
        &self,
        certificate_der: &[u8],
        private_key_der: &[u8],
        created_at: i64,
    ) -> Result<(), AppError> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO local_tls_credentials (singleton, certificate_der, private_key_der, created_at) VALUES (1, ?1, ?2, ?3)",
                params![certificate_der, private_key_der, created_at],
            )?;
            Ok(())
        })
    }
    /// This transaction runs only after the platform store has accepted and
    /// the identity-bound certificate has validated. A database failure leaves
    /// legacy bytes intact so startup can retry safely.
    pub(crate) fn clear_legacy_private_material(&self, migrated_at: i64) -> Result<(), AppError> {
        let mut connection = self
            .connection
            .lock()
            .expect("settings repository mutex poisoned");
        let transaction = connection.transaction().map_err(AppError::Database)?;
        transaction
            .execute("DELETE FROM local_identity WHERE singleton = 1", [])
            .map_err(AppError::Database)?;
        transaction
            .execute("DELETE FROM local_tls_credentials WHERE singleton = 1", [])
            .map_err(AppError::Database)?;
        transaction
            .execute(
                "INSERT OR REPLACE INTO secret_store_migrations (name, migrated_at) VALUES ('v10-os-secret-store', ?1)",
                [migrated_at],
            )
            .map_err(AppError::Database)?;
        transaction.commit().map_err(AppError::Database)
    }
    pub fn pending_pairings(&self) -> Result<Vec<PendingPairing>, AppError> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare("SELECT id, device_id, public_key, certificate_fingerprint, remote_name, created_at, expires_at, local_confirmed, remote_confirmed, sas_code FROM pending_pairings ORDER BY created_at")?;
            let pairings = statement.query_map([], pending_pairing_from_row)?.collect();
            pairings
        })
    }
    pub fn save_pending_pairing(&self, pairing: &PendingPairing) -> Result<(), AppError> {
        self.with_connection(|connection| { connection.execute("INSERT INTO pending_pairings (id, device_id, public_key, certificate_fingerprint, remote_name, created_at, expires_at, local_confirmed, remote_confirmed, sas_code) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)", params![pairing.id, pairing.device_id, pairing.public_key, pairing.certificate_fingerprint, pairing.remote_name, pairing.created_at, pairing.expires_at, pairing.local_confirmed as i64, pairing.remote_confirmed as i64, pairing.sas_code])?; Ok(()) })
    }
    pub fn delete_pending_pairing(&self, pairing_id: &str) -> Result<(), AppError> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM pending_pairings WHERE id = ?1", [pairing_id])?;
            Ok(())
        })
    }
    pub fn delete_expired_pairings(&self, now: i64) -> Result<(), AppError> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM pending_pairings WHERE expires_at <= ?1", [now])?;
            Ok(())
        })
    }
    pub fn mark_pairing_confirmation(
        &self,
        pairing_id: &str,
        local: bool,
        now: i64,
    ) -> Result<(), AppError> {
        self.with_connection(|connection| {
            let column = if local {
                "local_confirmed"
            } else {
                "remote_confirmed"
            };
            let changed = connection.execute(
                &format!(
                    "UPDATE pending_pairings SET {column} = 1 WHERE id = ?1 AND expires_at > ?2"
                ),
                params![pairing_id, now],
            )?;
            if changed == 0 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            Ok(())
        })
        .map_err(|error| match error {
            AppError::Database(rusqlite::Error::QueryReturnedNoRows) => AppError::Validation {
                code: "pairing_not_found",
                message: "That pairing request is no longer available.",
                field: Some("pairingId"),
            },
            other => other,
        })
    }
    pub fn commit_confirmed_pairing(
        &self,
        pairing_id: &str,
        peer: &TrustedPeer,
        now: i64,
    ) -> Result<(), AppError> {
        let mut connection = self
            .connection
            .lock()
            .expect("settings repository mutex poisoned");
        let transaction = connection.transaction().map_err(AppError::Database)?;
        let persisted = transaction
            .query_row(
                "SELECT device_id, public_key, expires_at, local_confirmed, remote_confirmed FROM pending_pairings WHERE id = ?1",
                [pairing_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?, row.get::<_, i64>(3)?, row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(AppError::Database)?
            .ok_or(AppError::Validation {
                code: "pairing_not_found",
                message: "That pairing request is no longer available.",
                field: Some("pairingId"),
            })?;
        if persisted.2 <= now {
            transaction
                .execute("DELETE FROM pending_pairings WHERE id = ?1", [pairing_id])
                .map_err(AppError::Database)?;
            transaction.commit().map_err(AppError::Database)?;
            return Err(AppError::Validation {
                code: "pairing_expired",
                message: "That pairing request has expired. Start a new pairing request.",
                field: Some("pairingId"),
            });
        }
        if persisted.3 == 0
            || persisted.4 == 0
            || persisted.0 != peer.device_id
            || persisted.1 != peer.public_key
        {
            return Err(AppError::Validation {
                code: "invalid_pairing",
                message: "The pairing request is invalid.",
                field: Some("pairingId"),
            });
        }
        transaction.execute("INSERT INTO trusted_peers (device_id, public_key, certificate_fingerprint, remote_name, local_alias, paired_at, last_seen_at, auto_send, revoked_at, endpoint) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9) ON CONFLICT(device_id) DO UPDATE SET public_key = excluded.public_key, certificate_fingerprint = excluded.certificate_fingerprint, remote_name = excluded.remote_name, local_alias = excluded.local_alias, paired_at = excluded.paired_at, last_seen_at = excluded.last_seen_at, auto_send = excluded.auto_send, revoked_at = NULL, endpoint = excluded.endpoint", params![peer.device_id, peer.public_key, peer.certificate_fingerprint, peer.remote_name, peer.local_alias, peer.paired_at, peer.last_seen_at, peer.auto_send as i64, peer.endpoint]).map_err(AppError::Database)?;
        transaction
            .execute("DELETE FROM pending_pairings WHERE id = ?1", [pairing_id])
            .map_err(AppError::Database)?;
        transaction.commit().map_err(AppError::Database)
    }

    pub fn save_batch(&self, batch: &Batch) -> Result<(), AppError> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO batches (id, direction, state, created_at, completed_at, total_bytes, total_entries, warning_count, wait_for_available) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) ON CONFLICT(id) DO UPDATE SET direction = excluded.direction, state = excluded.state, created_at = excluded.created_at, completed_at = excluded.completed_at, total_bytes = excluded.total_bytes, total_entries = excluded.total_entries, warning_count = excluded.warning_count, wait_for_available = excluded.wait_for_available",
                params![batch.id, batch.direction, batch.state, batch.created_at, batch.completed_at, batch.total_bytes, batch.total_entries, batch.warning_count, batch.wait_for_available as i64],
            )?;
            Ok(())
        })
    }

    pub fn batch(&self, id: &str) -> Result<Option<Batch>, AppError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, direction, state, created_at, completed_at, total_bytes, total_entries, warning_count, wait_for_available FROM batches WHERE id = ?1",
                    [id],
                    batch_from_row,
                )
                .optional()
        })
    }

    pub fn save_batch_target(&self, target: &BatchTarget) -> Result<(), AppError> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO batch_targets (id, batch_id, peer_device_id, state, acknowledged_bytes, error_code, retry_at, retry_count, wait_for_available) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) ON CONFLICT(id) DO UPDATE SET batch_id = excluded.batch_id, peer_device_id = excluded.peer_device_id, state = excluded.state, acknowledged_bytes = excluded.acknowledged_bytes, error_code = excluded.error_code, retry_at = excluded.retry_at, retry_count = excluded.retry_count, wait_for_available = excluded.wait_for_available",
                params![target.id, target.batch_id, target.peer_device_id, target.state, target.acknowledged_bytes, target.error_code, target.retry_at, target.retry_count, target.wait_for_available as i64],
            )?;
            Ok(())
        })
    }

    pub fn batch_target(&self, id: &str) -> Result<Option<BatchTarget>, AppError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, batch_id, peer_device_id, state, acknowledged_bytes, error_code, retry_at, retry_count, wait_for_available FROM batch_targets WHERE id = ?1",
                    [id],
                    batch_target_from_row,
                )
                .optional()
        })
    }

    /// Ordered target snapshot used when deriving a batch terminal state.  The
    /// scheduler must never infer another peer's outcome from an in-memory
    /// worker result.
    pub fn batch_targets(&self, batch_id: &str) -> Result<Vec<BatchTarget>, AppError> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, batch_id, peer_device_id, state, acknowledged_bytes, error_code, retry_at, retry_count, wait_for_available FROM batch_targets WHERE batch_id = ?1 ORDER BY id",
            )?;
            let targets = statement
                .query_map([batch_id], batch_target_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(targets)
        })
    }

    pub fn save_item(&self, item: &TransferItem) -> Result<(), AppError> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO items (id, batch_id, parent_item_id, kind, display_name, source_path_local, destination_path_local, size, mtime, state, warning_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) ON CONFLICT(id) DO UPDATE SET batch_id = excluded.batch_id, parent_item_id = excluded.parent_item_id, kind = excluded.kind, display_name = excluded.display_name, source_path_local = excluded.source_path_local, destination_path_local = excluded.destination_path_local, size = excluded.size, mtime = excluded.mtime, state = excluded.state, warning_json = excluded.warning_json",
                params![item.id, item.batch_id, item.parent_item_id, item.kind, item.display_name, item.source_path_local, item.destination_path_local, item.size, item.mtime, item.state, item.warning_json],
            )?;
            Ok(())
        })
    }

    pub fn item(&self, id: &str) -> Result<Option<TransferItem>, AppError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, batch_id, parent_item_id, kind, display_name, source_path_local, destination_path_local, size, mtime, state, warning_json FROM items WHERE id = ?1",
                    [id],
                    transfer_item_from_row,
                )
                .optional()
        })
    }

    pub fn save_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), AppError> {
        #[cfg(test)]
        if self
            .fail_checkpoint_writes
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(AppError::Validation {
                code: "checkpoint_write_failed",
                message: "Checkpoint persistence was fault injected.",
                field: None,
            });
        }
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO checkpoints (target_id, item_id, durable_offset, verified_hash, updated_at) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(target_id, item_id) DO UPDATE SET durable_offset = excluded.durable_offset, verified_hash = excluded.verified_hash, updated_at = excluded.updated_at",
                params![checkpoint.target_id, checkpoint.item_id, checkpoint.durable_offset, checkpoint.verified_hash, checkpoint.updated_at],
            )?;
            Ok(())
        })
    }

    #[cfg(test)]
    pub fn fail_checkpoint_writes_for_test(&self, fail: bool) {
        self.fail_checkpoint_writes
            .store(fail, std::sync::atomic::Ordering::Release);
    }

    pub fn checkpoint(
        &self,
        target_id: &str,
        item_id: &str,
    ) -> Result<Option<Checkpoint>, AppError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT target_id, item_id, durable_offset, verified_hash, updated_at FROM checkpoints WHERE target_id = ?1 AND item_id = ?2",
                    params![target_id, item_id],
                    checkpoint_from_row,
                )
                .optional()
        })
    }
    /// Receiver-side durable acknowledgement for a stable protocol batch/item.
    /// The target is intentionally resolved in the database: it is receiver
    /// owned and must never be supplied by a reconnecting sender.
    pub fn incoming_checkpoint(
        &self,
        batch_id: &str,
        item_id: &str,
    ) -> Result<Option<Checkpoint>, AppError> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT c.target_id, c.item_id, c.durable_offset, c.verified_hash, c.updated_at FROM checkpoints c JOIN batch_targets t ON t.id = c.target_id WHERE t.batch_id = ?1 AND c.item_id = ?2 ORDER BY c.updated_at DESC LIMIT 1",
                params![batch_id, item_id], checkpoint_from_row,
            ).optional()
        })
    }

    pub fn enqueue_outgoing_batch(
        &self,
        batch: &Batch,
        targets: &[BatchTarget],
        items: &[TransferItem],
    ) -> Result<(), AppError> {
        let mut connection = self
            .connection
            .lock()
            .expect("settings repository mutex poisoned");
        let transaction = connection.transaction().map_err(AppError::Database)?;

        for target in targets {
            let trusted = transaction
                .query_row(
                    "SELECT 1 FROM trusted_peers WHERE device_id = ?1 AND revoked_at IS NULL",
                    [&target.peer_device_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(AppError::Database)?
                .is_some();
            if !trusted {
                return Err(AppError::Validation {
                    code: "unknown_recipient",
                    message: "One or more selected devices are not trusted recipients.",
                    field: Some("targetDeviceIds"),
                });
            }
        }

        transaction.execute(
            "INSERT INTO batches (id, direction, state, created_at, completed_at, total_bytes, total_entries, warning_count, wait_for_available) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![batch.id, batch.direction, batch.state, batch.created_at, batch.completed_at, batch.total_bytes, batch.total_entries, batch.warning_count, batch.wait_for_available as i64],
        ).map_err(AppError::Database)?;
        for target in targets {
            transaction.execute(
                "INSERT INTO batch_targets (id, batch_id, peer_device_id, state, acknowledged_bytes, error_code, retry_at, retry_count, wait_for_available) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![target.id, target.batch_id, target.peer_device_id, target.state, target.acknowledged_bytes, target.error_code, target.retry_at, target.retry_count, target.wait_for_available as i64],
            ).map_err(AppError::Database)?;
        }
        for item in items {
            transaction.execute(
                "INSERT INTO items (id, batch_id, parent_item_id, kind, display_name, source_path_local, destination_path_local, size, mtime, state, warning_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![item.id, item.batch_id, item.parent_item_id, item.kind, item.display_name, item.source_path_local, item.destination_path_local, item.size, item.mtime, item.state, item.warning_json],
            ).map_err(AppError::Database)?;
        }
        transaction.commit().map_err(AppError::Database)
    }

    pub fn outgoing_batches(&self) -> Result<Vec<PersistedBatch>, AppError> {
        self.batches_by_direction("outgoing")
    }

    /// Includes both outgoing and authenticated incoming batches for an
    /// honest activity/snapshot view.
    pub fn all_batches(&self) -> Result<Vec<PersistedBatch>, AppError> {
        self.with_connection(|connection| {
            let mut batch_statement = connection.prepare(
                "SELECT id, direction, state, created_at, completed_at, total_bytes, total_entries, warning_count, wait_for_available FROM batches ORDER BY created_at DESC, id DESC",
            )?;
            let batches = batch_statement.query_map([], batch_from_row)?.collect::<Result<Vec<_>, _>>()?;
            hydrate_batches(connection, batches)
        })
    }
    /// History retention removes only durable metadata; finalized receive files
    /// remain untouched at their user-selected destination.
    pub fn prune_terminal_history_before(&self, cutoff: i64) -> Result<(), AppError> {
        self.with_connection(|connection| {
            connection.execute(
                "DELETE FROM batches WHERE state IN ('completed', 'failed', 'cancelled') AND created_at < ?1",
                [cutoff],
            )?;
            Ok(())
        })
    }

    /// Atomically claim one terminal incoming batch for a privacy-safe desktop
    /// notification. A persisted ledger prevents restart/history replays.
    pub fn claim_incoming_notification(&self, batch_id: &str, now: i64) -> Result<bool, AppError> {
        let connection = self
            .connection
            .lock()
            .expect("settings repository mutex poisoned");
        let changed = connection.execute(
            "INSERT OR IGNORE INTO incoming_notification_ledger (batch_id, notified_at) VALUES (?1, ?2)",
            params![batch_id, now],
        ).map_err(AppError::Database)?;
        Ok(changed == 1)
    }
    fn batches_by_direction(&self, direction: &str) -> Result<Vec<PersistedBatch>, AppError> {
        self.with_connection(|connection| {
            let mut batch_statement = connection.prepare(
                "SELECT id, direction, state, created_at, completed_at, total_bytes, total_entries, warning_count, wait_for_available FROM batches WHERE direction = ?1 ORDER BY created_at DESC, id DESC",
            )?;
            let batches = batch_statement
                .query_map([direction], batch_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            hydrate_batches(connection, batches)
        })
    }

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, AppError> {
        let connection = self
            .connection
            .lock()
            .expect("settings repository mutex poisoned");
        operation(&connection).map_err(AppError::Database)
    }
}

fn hydrate_batches(
    connection: &Connection,
    batches: Vec<Batch>,
) -> rusqlite::Result<Vec<PersistedBatch>> {
    batches.into_iter().map(|batch| {
        let mut target_statement = connection.prepare("SELECT id, batch_id, peer_device_id, state, acknowledged_bytes, error_code, retry_at, retry_count, wait_for_available FROM batch_targets WHERE batch_id = ?1 ORDER BY id")?;
        let targets = target_statement.query_map([&batch.id], batch_target_from_row)?.collect::<Result<Vec<_>, _>>()?;
        let mut item_statement = connection.prepare("SELECT id, batch_id, parent_item_id, kind, display_name, source_path_local, destination_path_local, size, mtime, state, warning_json FROM items WHERE batch_id = ?1 ORDER BY id")?;
        let items = item_statement.query_map([&batch.id], transfer_item_from_row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(PersistedBatch { batch, targets, items })
    }).collect()
}

fn apply_migrations(connection: &mut Connection) -> Result<(), AppError> {
    let transaction = connection.transaction().map_err(AppError::Database)?;
    transaction
        .execute_batch(INITIAL_MIGRATION)
        .map_err(AppError::Database)?;
    let version: String = transaction
        .query_row(
            "SELECT value FROM schema_metadata WHERE key = 'migration_version'",
            [],
            |row| row.get(0),
        )
        .map_err(AppError::Database)?;
    let version = version
        .parse::<i64>()
        .map_err(|_| AppError::Database(rusqlite::Error::InvalidQuery))?;
    if version < 2 {
        transaction
            .execute_batch(TRANSFER_HISTORY_MIGRATION)
            .map_err(AppError::Database)?;
        transaction
            .execute(
                "UPDATE schema_metadata SET value = '2' WHERE key = 'migration_version'",
                [],
            )
            .map_err(AppError::Database)?;
    }
    if version < 3 {
        transaction
            .execute_batch(IDENTITY_PAIRING_MIGRATION)
            .map_err(AppError::Database)?;
        transaction
            .execute(
                "UPDATE schema_metadata SET value = '3' WHERE key = 'migration_version'",
                [],
            )
            .map_err(AppError::Database)?;
    }
    if version < 4 {
        transaction
            .execute_batch(LISTENER_ADDRESS_MIGRATION)
            .map_err(AppError::Database)?;
        transaction
            .execute(
                "UPDATE schema_metadata SET value = '4' WHERE key = 'migration_version'",
                [],
            )
            .map_err(AppError::Database)?;
    }
    if version < 5 {
        transaction
            .execute_batch(TLS_CREDENTIALS_AND_ENDPOINTS_MIGRATION)
            .map_err(AppError::Database)?;
        transaction
            .execute(
                "UPDATE schema_metadata SET value = '5' WHERE key = 'migration_version'",
                [],
            )
            .map_err(AppError::Database)?;
    }
    if version < 6 {
        transaction
            .execute_batch(PAIRING_REMOTE_CONFIRMATION_MIGRATION)
            .map_err(AppError::Database)?;
        transaction
            .execute(
                "UPDATE schema_metadata SET value = '6' WHERE key = 'migration_version'",
                [],
            )
            .map_err(AppError::Database)?;
    }
    if version < 7 {
        transaction
            .execute_batch(PAIRING_SAS_MIGRATION)
            .map_err(AppError::Database)?;
        transaction
            .execute(
                "UPDATE schema_metadata SET value = '7' WHERE key = 'migration_version'",
                [],
            )
            .map_err(AppError::Database)?;
    }
    if version < 8 {
        transaction
            .execute_batch(SEND_WHEN_AVAILABLE_MIGRATION)
            .map_err(AppError::Database)?;
        transaction
            .execute(
                "UPDATE schema_metadata SET value = '8' WHERE key = 'migration_version'",
                [],
            )
            .map_err(AppError::Database)?;
    }
    if version < 9 {
        transaction
            .execute_batch(NOTIFICATION_LEDGER_MIGRATION)
            .map_err(AppError::Database)?;
        transaction
            .execute(
                "UPDATE schema_metadata SET value = '9' WHERE key = 'migration_version'",
                [],
            )
            .map_err(AppError::Database)?;
    }
    if version < 10 {
        transaction
            .execute_batch(OS_SECRET_STORE_MIGRATION)
            .map_err(AppError::Database)?;
        transaction
            .execute(
                "UPDATE schema_metadata SET value = '10' WHERE key = 'migration_version'",
                [],
            )
            .map_err(AppError::Database)?;
    }
    if version < 11 {
        transaction
            .execute_batch(SETTINGS_CONTRACT_MIGRATION)
            .map_err(AppError::Database)?;
        transaction
            .execute(
                "UPDATE schema_metadata SET value = '11' WHERE key = 'migration_version'",
                [],
            )
            .map_err(AppError::Database)?;
    }
    if version < 12 {
        transaction
            .execute_batch(AUTOMATIC_DEVICE_TRUST_MIGRATION)
            .map_err(AppError::Database)?;
        transaction
            .execute(
                "UPDATE schema_metadata SET value = '12' WHERE key = 'migration_version'",
                [],
            )
            .map_err(AppError::Database)?;
    }
    if version < 13 {
        transaction
            .execute_batch(LAN_LISTENER_DEFAULT_MIGRATION)
            .map_err(AppError::Database)?;
        transaction
            .execute(
                "UPDATE schema_metadata SET value = '13' WHERE key = 'migration_version'",
                [],
            )
            .map_err(AppError::Database)?;
    }
    transaction.commit().map_err(AppError::Database)
}

fn trusted_peer_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TrustedPeer> {
    Ok(TrustedPeer {
        device_id: row.get(0)?,
        public_key: row.get(1)?,
        certificate_fingerprint: row.get(2)?,
        remote_name: row.get(3)?,
        local_alias: row.get(4)?,
        paired_at: row.get(5)?,
        last_seen_at: row.get(6)?,
        auto_send: row.get::<_, i64>(7)? != 0,
        revoked_at: row.get(8)?,
        endpoint: row.get(9)?,
    })
}
fn pending_pairing_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PendingPairing> {
    Ok(PendingPairing {
        id: row.get(0)?,
        device_id: row.get(1)?,
        public_key: row.get(2)?,
        certificate_fingerprint: row.get(3)?,
        remote_name: row.get(4)?,
        created_at: row.get(5)?,
        expires_at: row.get(6)?,
        local_confirmed: row.get::<_, i64>(7)? != 0,
        remote_confirmed: row.get::<_, i64>(8)? != 0,
        sas_code: row.get(9)?,
    })
}

fn batch_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Batch> {
    Ok(Batch {
        id: row.get(0)?,
        direction: row.get(1)?,
        state: row.get(2)?,
        created_at: row.get(3)?,
        completed_at: row.get(4)?,
        total_bytes: row.get(5)?,
        total_entries: row.get(6)?,
        warning_count: row.get(7)?,
        wait_for_available: row.get::<_, i64>(8)? != 0,
    })
}

fn batch_target_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BatchTarget> {
    Ok(BatchTarget {
        id: row.get(0)?,
        batch_id: row.get(1)?,
        peer_device_id: row.get(2)?,
        state: row.get(3)?,
        acknowledged_bytes: row.get(4)?,
        error_code: row.get(5)?,
        retry_at: row.get(6)?,
        retry_count: row.get(7)?,
        wait_for_available: row.get::<_, i64>(8)? != 0,
    })
}

fn transfer_item_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TransferItem> {
    Ok(TransferItem {
        id: row.get(0)?,
        batch_id: row.get(1)?,
        parent_item_id: row.get(2)?,
        kind: row.get(3)?,
        display_name: row.get(4)?,
        source_path_local: row.get(5)?,
        destination_path_local: row.get(6)?,
        size: row.get(7)?,
        mtime: row.get(8)?,
        state: row.get(9)?,
        warning_json: row.get(10)?,
    })
}

fn checkpoint_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Checkpoint> {
    Ok(Checkpoint {
        target_id: row.get(0)?,
        item_id: row.get(1)?,
        durable_offset: row.get(2)?,
        verified_hash: row.get(3)?,
        updated_at: row.get(4)?,
    })
}
#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // The desktop bootstrap selects its application-data location.
pub fn default_database_path(app_data: &Path) -> PathBuf {
    app_data.join("fileporter.sqlite3")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_creates_safe_default_settings() {
        let d = tempfile::tempdir().unwrap();
        let repo = SettingsRepository::open(d.path().join("settings.sqlite3")).unwrap();
        assert_eq!(repo.load().unwrap(), Settings::default());
        assert_eq!(repo.load().unwrap().history_retention_days, 30);
        assert!(repo.load().unwrap().automatic_device_trust);
        assert_eq!(repo.migration_version().unwrap(), 13);
        assert_eq!(repo.load().unwrap().listen_address, "0.0.0.0:0");
    }
    #[test]
    fn migration_replaces_only_the_old_loopback_default() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("settings.sqlite3");
        let repo = SettingsRepository::open(db.clone()).unwrap();
        repo.connection
            .lock()
            .unwrap()
            .execute_batch(
                "UPDATE settings SET listen_address = '127.0.0.1:0';
                 UPDATE schema_metadata SET value = '12' WHERE key = 'migration_version';",
            )
            .unwrap();
        drop(repo);

        let migrated = SettingsRepository::open(db).unwrap();
        assert_eq!(migrated.load().unwrap().listen_address, "0.0.0.0:0");
        assert_eq!(migrated.migration_version().unwrap(), 13);
    }
    #[test]
    fn settings_updates_survive_reopening() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("settings.sqlite3");
        let repo = SettingsRepository::open(db.clone()).unwrap();
        let value = Settings {
            device_name: "Studio Mac".into(),
            onboarding_complete: true,
            automatic_device_trust: false,
            ..Settings::default()
        };
        repo.save(&value).unwrap();
        drop(repo);
        assert_eq!(SettingsRepository::open(db).unwrap().load().unwrap(), value);
    }

    #[test]
    fn transfer_records_survive_reopening() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("transfer.sqlite3");
        let repository = SettingsRepository::open(database.clone()).unwrap();

        let peer = TrustedPeer {
            device_id: "peer-1".into(),
            public_key: vec![1, 2, 3],
            certificate_fingerprint: "sha256:peer-1".into(),
            remote_name: "Studio Mac".into(),
            local_alias: Some("Studio".into()),
            paired_at: 1_700_000_000,
            last_seen_at: Some(1_700_000_100),
            auto_send: true,
            revoked_at: None,
            endpoint: Some("127.0.0.1:4242".into()),
        };
        let batch = Batch {
            id: "batch-1".into(),
            direction: "outgoing".into(),
            state: "paused".into(),
            created_at: 1_700_000_200,
            completed_at: None,
            total_bytes: 42,
            total_entries: 1,
            warning_count: 0,
            wait_for_available: true,
        };
        let target = BatchTarget {
            id: "target-1".into(),
            batch_id: batch.id.clone(),
            peer_device_id: peer.device_id.clone(),
            state: "paused".into(),
            acknowledged_bytes: 21,
            error_code: None,
            retry_at: Some(1_700_000_300),
            retry_count: 2,
            wait_for_available: true,
        };
        let item = TransferItem {
            id: "item-1".into(),
            batch_id: batch.id.clone(),
            parent_item_id: None,
            kind: "file".into(),
            display_name: "notes.txt".into(),
            source_path_local: Some("C:/source/notes.txt".into()),
            destination_path_local: None,
            size: 42,
            mtime: Some(1_700_000_190),
            state: "streaming".into(),
            warning_json: None,
        };
        let checkpoint = Checkpoint {
            target_id: target.id.clone(),
            item_id: item.id.clone(),
            durable_offset: 21,
            verified_hash: Some(vec![9, 8, 7]),
            updated_at: 1_700_000_250,
        };

        repository.upsert_trusted_peer(&peer).unwrap();
        repository.save_batch(&batch).unwrap();
        repository.save_batch_target(&target).unwrap();
        repository.save_item(&item).unwrap();
        repository.save_checkpoint(&checkpoint).unwrap();
        drop(repository);

        let reopened = SettingsRepository::open(database).unwrap();
        let connection = reopened.connection.lock().unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT value FROM schema_metadata WHERE key = 'migration_version'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "13"
        );
        drop(connection);
        assert_eq!(reopened.trusted_peer(&peer.device_id).unwrap(), Some(peer));
        assert_eq!(reopened.batch(&batch.id).unwrap(), Some(batch));
        assert_eq!(reopened.batch_target(&target.id).unwrap(), Some(target));
        assert_eq!(reopened.item(&item.id).unwrap(), Some(item));
        assert_eq!(
            reopened
                .checkpoint(&checkpoint.target_id, &checkpoint.item_id)
                .unwrap(),
            Some(checkpoint)
        );
    }

    #[test]
    fn migration_keeps_existing_settings_and_applies_busy_timeout() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("existing.sqlite3");
        let connection = Connection::open(&database).unwrap();
        connection.execute_batch(INITIAL_MIGRATION).unwrap();
        connection
            .execute(
                "UPDATE settings SET device_name = 'Already configured' WHERE singleton = 1",
                [],
            )
            .unwrap();
        drop(connection);

        let repository = SettingsRepository::open(database).unwrap();
        assert_eq!(repository.load().unwrap().device_name, "Already configured");
        let connection = repository.connection.lock().unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, u64>(0))
                .unwrap(),
            BUSY_TIMEOUT_MS
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT value FROM schema_metadata WHERE key = 'migration_version'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "13"
        );
    }

    #[test]
    fn v4_database_migrates_without_losing_peers_or_inventing_endpoints() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("v4.sqlite3");
        let connection = Connection::open(&database).unwrap();
        connection.execute_batch(INITIAL_MIGRATION).unwrap();
        connection
            .execute_batch(TRANSFER_HISTORY_MIGRATION)
            .unwrap();
        connection
            .execute_batch(IDENTITY_PAIRING_MIGRATION)
            .unwrap();
        connection
            .execute_batch(LISTENER_ADDRESS_MIGRATION)
            .unwrap();
        connection
            .execute(
                "UPDATE schema_metadata SET value = '4' WHERE key = 'migration_version'",
                [],
            )
            .unwrap();
        connection.execute("INSERT INTO trusted_peers (device_id, public_key, certificate_fingerprint, remote_name, paired_at, auto_send) VALUES ('legacy', X'01', 'legacy', 'Legacy', 1, 0)", []).unwrap();
        drop(connection);
        let repository = SettingsRepository::open(database).unwrap();
        let peer = repository.trusted_peer("legacy").unwrap().unwrap();
        assert_eq!(peer.endpoint, None);
        assert!(repository.legacy_tls_credentials().unwrap().is_none());
    }

    #[test]
    fn trusted_peer_rejects_public_endpoint() {
        let directory = tempfile::tempdir().unwrap();
        let repository = SettingsRepository::open(directory.path().join("db.sqlite3")).unwrap();
        let peer = TrustedPeer {
            device_id: "peer".into(),
            public_key: vec![1],
            certificate_fingerprint: "legacy".into(),
            remote_name: "Peer".into(),
            local_alias: None,
            paired_at: 1,
            last_seen_at: None,
            auto_send: false,
            revoked_at: None,
            endpoint: Some("8.8.8.8:4242".into()),
        };
        assert!(matches!(
            repository.upsert_trusted_peer(&peer),
            Err(AppError::Validation {
                code: "invalid_endpoint",
                ..
            })
        ));
    }
}
