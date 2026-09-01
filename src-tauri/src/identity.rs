//! Local-only identity and pairing coordination.  This module deliberately has
//! no transport dependency and never represents reachability or network proof.

use std::{
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use fileporter_identity::{
    DeviceIdentity, DevicePublicIdentity, PairingError, PairingSession, PairingState,
};
use fileporter_network::LocalCertificate;
use serde::Serialize;

use crate::{
    error::AppError,
    persistence::{PendingPairing, SettingsRepository, TrustedPeer},
    secret_store::{PlatformSecretStore, SecretStore},
};

pub(crate) const PAIRING_LIFETIME: Duration = Duration::from_secs(120);

pub trait IdentitySecretStore: Send + Sync {
    fn load_or_create(&self, now: i64) -> Result<DeviceIdentity, AppError>;
    fn load_or_create_tls_certificate(
        &self,
        identity: &DeviceIdentity,
        now: i64,
    ) -> Result<LocalCertificate, AppError>;
}

const IDENTITY_SECRET_NAME: &str = "identity-ed25519-v1";
const TLS_SECRET_NAME: &str = "tls-ed25519-v1";

/// Production private-material boundary. SQLite is consulted only to import a
/// v9-or-earlier record, then is cleared; the platform credential store owns
/// all newly created identity and TLS private bytes.
pub struct DesktopIdentityStore {
    repository: std::sync::Arc<SettingsRepository>,
    secrets: std::sync::Arc<dyn SecretStore>,
}
impl DesktopIdentityStore {
    pub(crate) fn with_secret_store(
        repository: std::sync::Arc<SettingsRepository>,
        secrets: std::sync::Arc<dyn SecretStore>,
    ) -> Self {
        Self {
            repository,
            secrets,
        }
    }

    fn migrate_legacy(&self, now: i64) -> Result<(), AppError> {
        let legacy_identity = self.repository.legacy_identity_secret()?;
        let legacy_tls = self.repository.legacy_tls_credentials()?;
        if legacy_identity.is_none() && legacy_tls.is_none() {
            return Ok(());
        }

        let identity_bytes = match legacy_identity.as_deref() {
            Some(bytes) => bytes
                .try_into()
                .map_err(|_| AppError::IdentityStorageInvalid)?,
            None => self
                .secrets
                .get(IDENTITY_SECRET_NAME)?
                .ok_or(AppError::IdentityStorageInvalid)?
                .try_into()
                .map_err(|_| AppError::IdentityStorageInvalid)?,
        };
        let identity = DeviceIdentity::from_secret_bytes(identity_bytes);
        let tls_bytes = legacy_tls
            .as_ref()
            .map(|(certificate_der, private_key_der)| {
                let certificate = LocalCertificate::from_persisted_der(
                    &identity,
                    certificate_der.clone(),
                    private_key_der.clone(),
                )
                .map_err(|_| AppError::IdentityStorageInvalid)?;
                Ok::<_, AppError>(encode_tls_secret(&certificate.persisted_der()))
            })
            .transpose()?;

        if let Some(bytes) = legacy_identity.as_deref() {
            self.write_once(IDENTITY_SECRET_NAME, bytes)?;
        }
        if let Some(bytes) = tls_bytes.as_deref() {
            self.write_once(TLS_SECRET_NAME, bytes)?;
        }
        // Credential-store writes happen first. This transaction makes legacy
        // deletion and the v10 marker all-or-nothing and therefore retryable.
        self.repository.clear_legacy_private_material(now)
    }

    fn write_once(&self, name: &str, value: &[u8]) -> Result<(), AppError> {
        match self.secrets.get(name)? {
            Some(existing) if existing == value => Ok(()),
            Some(_) => Err(AppError::IdentityStorageInvalid),
            None => self.secrets.set(name, value),
        }
    }
}
impl IdentitySecretStore for DesktopIdentityStore {
    fn load_or_create(&self, now: i64) -> Result<DeviceIdentity, AppError> {
        self.migrate_legacy(now)?;
        if let Some(secret) = self.secrets.get(IDENTITY_SECRET_NAME)? {
            let bytes: [u8; 32] = secret
                .try_into()
                .map_err(|_| AppError::IdentityStorageInvalid)?;
            return Ok(DeviceIdentity::from_secret_bytes(bytes));
        }
        let identity = DeviceIdentity::generate();
        let secret = identity.export_secret_bytes();
        self.secrets.set(IDENTITY_SECRET_NAME, &secret[..])?;
        Ok(identity)
    }
    fn load_or_create_tls_certificate(
        &self,
        identity: &DeviceIdentity,
        now: i64,
    ) -> Result<LocalCertificate, AppError> {
        self.migrate_legacy(now)?;
        if let Some(serialized) = self.secrets.get(TLS_SECRET_NAME)? {
            let (certificate_der, private_key_der) = decode_tls_secret(&serialized)?;
            return LocalCertificate::from_persisted_der(
                identity,
                certificate_der,
                private_key_der,
            )
            .map_err(|_| AppError::IdentityStorageInvalid);
        }
        let certificate =
            LocalCertificate::generate(identity).map_err(|_| AppError::IdentityStorageInvalid)?;
        let (certificate_der, private_key_der) = certificate.persisted_der();
        self.secrets.set(
            TLS_SECRET_NAME,
            &encode_tls_secret(&(certificate_der, private_key_der)),
        )?;
        Ok(certificate)
    }
}

fn encode_tls_secret(value: &(Vec<u8>, Vec<u8>)) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(4 + value.0.len() + value.1.len());
    encoded.extend_from_slice(&(value.0.len() as u32).to_be_bytes());
    encoded.extend_from_slice(&value.0);
    encoded.extend_from_slice(&value.1);
    encoded
}

fn decode_tls_secret(value: &[u8]) -> Result<(Vec<u8>, Vec<u8>), AppError> {
    let length = value
        .get(..4)
        .and_then(|prefix| prefix.try_into().ok())
        .map(u32::from_be_bytes)
        .ok_or(AppError::IdentityStorageInvalid)? as usize;
    let certificate_end = 4usize
        .checked_add(length)
        .filter(|end| *end < value.len())
        .ok_or(AppError::IdentityStorageInvalid)?;
    Ok((
        value[4..certificate_end].to_vec(),
        value[certificate_end..].to_vec(),
    ))
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PendingPairingView {
    pub id: String,
    pub device_id: String,
    pub remote_name: String,
    pub certificate_fingerprint: String,
    pub expires_at: i64,
    pub local_confirmed: bool,
    pub remote_confirmed: bool,
    pub sas_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrustedDeviceView {
    pub device_id: String,
    pub name: String,
    pub alias: Option<String>,
    pub paired_at: i64,
    pub last_seen_at: Option<i64>,
    pub certificate_fingerprint_short: String,
    pub auto_send: bool,
    pub endpoint: Option<String>,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairingConfirmationView {
    pub pairing: PendingPairingView,
    pub trusted_device: Option<TrustedDeviceView>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairingSnapshot {
    pub local_device_id: String,
    pub pending_pairings: Vec<PendingPairingView>,
    pub trusted_devices: Vec<TrustedDeviceView>,
}

pub struct PairingCoordinator {
    repository: std::sync::Arc<SettingsRepository>,
    identity: DeviceIdentity,
    local_certificate: LocalCertificate,
    sessions: Mutex<Vec<(PendingPairing, PairingSession)>>,
}

impl PairingCoordinator {
    /// Public-only material suitable for LAN service metadata. The private
    /// identity key and certificate bytes remain inside this coordinator.
    #[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Used by desktop listener/discovery startup only.
    pub(crate) fn discovery_identity(&self) -> (String, String) {
        (
            self.identity.public_identity().device_id(),
            format!(
                "blake3:{}",
                hex::encode(self.local_certificate.fingerprint())
            ),
        )
    }
    pub fn open(repository: std::sync::Arc<SettingsRepository>) -> Result<Self, AppError> {
        Self::open_with_store(repository, std::sync::Arc::new(PlatformSecretStore))
    }
    fn open_with_store(
        repository: std::sync::Arc<SettingsRepository>,
        secrets: std::sync::Arc<dyn SecretStore>,
    ) -> Result<Self, AppError> {
        let now = unix_now();
        let store = DesktopIdentityStore::with_secret_store(repository.clone(), secrets);
        let identity = store.load_or_create(now)?;
        let local_certificate = store.load_or_create_tls_certificate(&identity, now)?;
        repository.delete_expired_pairings(now)?;
        let mut sessions = Vec::new();
        for pairing in repository.pending_pairings()? {
            // A legacy row has no SAS and remains rejectable/expirable, but a
            // malformed non-null value is corrupt state and must not be shown
            // or allowed to participate in confirmation.
            if pairing
                .sas_code
                .as_deref()
                .is_some_and(|sas_code| !is_normalized_sas_code(sas_code))
            {
                repository.delete_pending_pairing(&pairing.id)?;
                continue;
            }
            let expiry = UNIX_EPOCH + Duration::from_secs(pairing.expires_at.max(0) as u64);
            let mut session = PairingSession::new(expiry);
            if pairing.local_confirmed {
                session
                    .confirm_local(
                        UNIX_EPOCH + Duration::from_secs(pairing.created_at.max(0) as u64),
                    )
                    .map_err(|_| invalid_pairing("pairingId"))?;
            }
            if pairing.remote_confirmed {
                session
                    .confirm_remote(
                        UNIX_EPOCH + Duration::from_secs(pairing.created_at.max(0) as u64),
                    )
                    .map_err(|_| invalid_pairing("pairingId"))?;
            }
            sessions.push((pairing, session));
        }
        Ok(Self {
            repository,
            identity,
            local_certificate,
            sessions: Mutex::new(sessions),
        })
    }
    #[cfg(test)]
    fn open_with_secret_store(
        repository: std::sync::Arc<SettingsRepository>,
        secrets: std::sync::Arc<dyn SecretStore>,
    ) -> Result<Self, AppError> {
        Self::open_with_store(repository, secrets)
    }

    /// Internal runtime use only; no command or snapshot can obtain this credential.
    pub(crate) fn local_certificate(&self) -> LocalCertificate {
        let (certificate_der, private_key_der) = self.local_certificate.persisted_der();
        LocalCertificate::from_persisted_der(&self.identity, certificate_der, private_key_der)
            .expect("validated local TLS credential must remain reloadable")
    }

    pub fn request(
        &self,
        remote_name: String,
        public_key: Vec<u8>,
        certificate_fingerprint: String,
    ) -> Result<PendingPairingView, AppError> {
        let remote_name = validate_text(&remote_name, "remoteName")?;
        let certificate_fingerprint = validate_fingerprint(&certificate_fingerprint)?;
        let key: [u8; 32] = public_key
            .try_into()
            .map_err(|_| invalid_pairing("publicKey"))?;
        let remote =
            DevicePublicIdentity::from_public_key(key).map_err(|_| invalid_pairing("publicKey"))?;
        if remote.public_key == self.identity.public_identity().public_key {
            return Err(invalid_pairing("publicKey"));
        }
        let now = unix_now();
        let pairing = PendingPairing {
            id: format!("pairing-{}-{}", remote.device_id(), now_nanos()),
            device_id: remote.device_id(),
            public_key: key.to_vec(),
            certificate_fingerprint,
            remote_name,
            created_at: now,
            expires_at: now + PAIRING_LIFETIME.as_secs() as i64,
            local_confirmed: false,
            remote_confirmed: false,
            sas_code: None,
        };
        self.repository.save_pending_pairing(&pairing)?;
        let expiry = UNIX_EPOCH + Duration::from_secs(pairing.expires_at as u64);
        self.sessions
            .lock()
            .expect("pairing mutex poisoned")
            .push((pairing.clone(), PairingSession::new(expiry)));
        Ok(view(&pairing))
    }
    /// Registers a peer only after the pairing TLS authentication and signed
    /// transcript have completed. The transport may confirm it automatically
    /// when authenticated local-network discovery is enabled.
    pub(crate) fn request_authenticated(
        &self,
        remote_name: String,
        peer: &fileporter_network::TrustedPeerPin,
        sas_code: String,
    ) -> Result<PendingPairingView, AppError> {
        let sas_code = validate_sas_code(&sas_code)?;
        let remote_name = validate_text(&remote_name, "remoteName")?;
        // A durable row is also the deny record for a forgotten device. Never
        // let background discovery silently clear revocation or replace a pin.
        if self.repository.trusted_peer(&peer.device_id)?.is_some() {
            return Err(invalid_pairing("publicKey"));
        }
        let now = unix_now();
        let pairing = PendingPairing {
            id: format!("pairing-{}-{}", peer.device_id, now_nanos()),
            device_id: peer.device_id.clone(),
            public_key: peer.public_key.to_vec(),
            certificate_fingerprint: format!(
                "blake3:{}",
                hex::encode(peer.certificate_fingerprint)
            ),
            remote_name,
            created_at: now,
            expires_at: now + PAIRING_LIFETIME.as_secs() as i64,
            local_confirmed: false,
            remote_confirmed: false,
            sas_code: Some(sas_code),
        };
        self.repository.save_pending_pairing(&pairing)?;
        let expiry = UNIX_EPOCH + Duration::from_secs(pairing.expires_at as u64);
        self.sessions
            .lock()
            .expect("pairing mutex poisoned")
            .push((pairing.clone(), PairingSession::new(expiry)));
        Ok(view(&pairing))
    }

    /// This local-only confirmation records a user-approved trust decision; it
    /// makes no claim that the device is online or mutually network-verified.
    pub fn confirm(&self, pairing_id: &str) -> Result<PairingConfirmationView, AppError> {
        let now = SystemTime::now();
        let now_unix = unix_now();
        let mut sessions = self.sessions.lock().expect("pairing mutex poisoned");
        let index = sessions
            .iter()
            .position(|(pairing, _)| pairing.id == pairing_id)
            .ok_or_else(pairing_not_found)?;
        let (pairing, session) = &mut sessions[index];
        if pairing.sas_code.is_none() {
            return Err(invalid_pairing("pairingId"));
        }
        match session.confirm_local(now) {
            Ok(PairingState::LocalConfirmed | PairingState::Confirmed) => {
                self.repository
                    .mark_pairing_confirmation(pairing_id, true, now_unix)?;
            }
            Err(PairingError::Expired) => {
                let id = pairing.id.clone();
                sessions.remove(index);
                self.repository.delete_pending_pairing(&id)?;
                return Err(pairing_expired());
            }
            _ => return Err(invalid_pairing("pairingId")),
        }
        if session.state() != PairingState::Confirmed {
            let mut pending = pairing.clone();
            pending.local_confirmed = true;
            return Ok(PairingConfirmationView {
                pairing: view(&pending),
                trusted_device: None,
            });
        }
        // Trust is committed only after both authenticated confirmations.
        let peer = TrustedPeer {
            device_id: pairing.device_id.clone(),
            public_key: pairing.public_key.clone(),
            certificate_fingerprint: pairing.certificate_fingerprint.clone(),
            remote_name: pairing.remote_name.clone(),
            local_alias: None,
            paired_at: now_unix,
            last_seen_at: None,
            auto_send: true,
            revoked_at: None,
            endpoint: None,
        };
        self.repository
            .commit_confirmed_pairing(&pairing.id, &peer, now_unix)?;
        let peer = trusted_view(&peer);
        let pending_view = view(pairing);
        sessions.remove(index);
        Ok(PairingConfirmationView {
            pairing: pending_view,
            trusted_device: Some(peer),
        })
    }
    /// Called only by the authenticated pairing transport after a matching
    /// `PairConfirmed` session frame.  It is not a Tauri command.
    pub fn confirm_remote(&self, pairing_id: &str) -> Result<Option<TrustedDeviceView>, AppError> {
        let now = SystemTime::now();
        let now_unix = unix_now();
        let mut sessions = self.sessions.lock().expect("pairing mutex poisoned");
        let index = sessions
            .iter()
            .position(|(pairing, _)| pairing.id == pairing_id)
            .ok_or_else(pairing_not_found)?;
        let (pairing, session) = &mut sessions[index];
        if pairing.sas_code.is_none() {
            return Err(invalid_pairing("pairingId"));
        }
        match session.confirm_remote(now) {
            Ok(_) => self
                .repository
                .mark_pairing_confirmation(pairing_id, false, now_unix)?,
            Err(PairingError::Expired) => {
                let id = pairing.id.clone();
                sessions.remove(index);
                self.repository.delete_pending_pairing(&id)?;
                return Err(pairing_expired());
            }
            Err(_) => return Err(invalid_pairing("pairingId")),
        }
        if session.state() != PairingState::Confirmed {
            return Ok(None);
        }
        let peer = TrustedPeer {
            device_id: pairing.device_id.clone(),
            public_key: pairing.public_key.clone(),
            certificate_fingerprint: pairing.certificate_fingerprint.clone(),
            remote_name: pairing.remote_name.clone(),
            local_alias: None,
            paired_at: now_unix,
            last_seen_at: None,
            auto_send: true,
            revoked_at: None,
            endpoint: None,
        };
        self.repository
            .commit_confirmed_pairing(&pairing.id, &peer, now_unix)?;
        sessions.remove(index);
        Ok(Some(trusted_view(&peer)))
    }

    pub(crate) fn automatic_device_trust_enabled(&self) -> bool {
        self.repository
            .load()
            .map(|settings| settings.automatic_device_trust)
            .unwrap_or(false)
    }
    pub fn reject(&self, pairing_id: &str) -> Result<(), AppError> {
        let mut sessions = self.sessions.lock().expect("pairing mutex poisoned");
        let index = sessions
            .iter()
            .position(|(pairing, _)| pairing.id == pairing_id)
            .ok_or_else(pairing_not_found)?;
        sessions[index].1.reject();
        let id = sessions.remove(index).0.id;
        self.repository.delete_pending_pairing(&id)
    }
    pub fn forget(&self, device_id: &str) -> Result<(), AppError> {
        self.repository.revoke_trusted_peer(device_id, unix_now())
    }
    pub fn snapshot(&self) -> Result<PairingSnapshot, AppError> {
        let now = SystemTime::now();
        let now_unix = unix_now();
        self.repository.delete_expired_pairings(now_unix)?;
        let mut sessions = self.sessions.lock().expect("pairing mutex poisoned");
        let expired = sessions
            .iter_mut()
            .filter_map(|(pairing, session)| {
                (session.expire_if_needed(now) == PairingState::Expired).then(|| pairing.id.clone())
            })
            .collect::<Vec<_>>();
        sessions.retain(|(_, session)| session.state() != PairingState::Expired);
        drop(sessions);
        for id in expired {
            self.repository.delete_pending_pairing(&id)?;
        }
        let pending_pairings = self
            .repository
            .pending_pairings()?
            .into_iter()
            .filter(|pairing| {
                pairing
                    .sas_code
                    .as_deref()
                    .map_or(true, is_normalized_sas_code)
            })
            .map(|pairing| view(&pairing))
            .collect();
        Ok(PairingSnapshot {
            local_device_id: self.identity.public_identity().device_id(),
            pending_pairings,
            trusted_devices: self
                .repository
                .active_trusted_peers()?
                .iter()
                .map(trusted_view)
                .collect(),
        })
    }
}
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
fn validate_text(value: &str, field: &'static str) -> Result<String, AppError> {
    let text = value.trim();
    if text.is_empty() || text.chars().count() > 128 {
        Err(invalid_pairing(field))
    } else {
        Ok(text.into())
    }
}
fn validate_fingerprint(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    let hex = value.strip_prefix("blake3:").unwrap_or(value);
    let bytes = hex::decode(hex).map_err(|_| invalid_pairing("certificateFingerprint"))?;
    if bytes.len() != 32 {
        return Err(invalid_pairing("certificateFingerprint"));
    }
    Ok(format!("blake3:{}", hex.to_ascii_lowercase()))
}
fn validate_sas_code(value: &str) -> Result<String, AppError> {
    if !is_normalized_sas_code(value) {
        return Err(invalid_pairing("sasCode"));
    }
    Ok(value.into())
}
fn is_normalized_sas_code(value: &str) -> bool {
    value.len() == 7
        && value.as_bytes().get(3) == Some(&b' ')
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| index == 3 || byte.is_ascii_digit())
}
fn invalid_pairing(field: &'static str) -> AppError {
    AppError::Validation {
        code: "invalid_pairing",
        message: "The pairing request is invalid.",
        field: Some(field),
    }
}
fn pairing_not_found() -> AppError {
    AppError::Validation {
        code: "pairing_not_found",
        message: "That pairing request is no longer available.",
        field: Some("pairingId"),
    }
}
fn pairing_expired() -> AppError {
    AppError::Validation {
        code: "pairing_expired",
        message: "That pairing request has expired. Start a new pairing request.",
        field: Some("pairingId"),
    }
}
fn view(pairing: &PendingPairing) -> PendingPairingView {
    PendingPairingView {
        id: pairing.id.clone(),
        device_id: pairing.device_id.clone(),
        remote_name: pairing.remote_name.clone(),
        certificate_fingerprint: pairing.certificate_fingerprint.clone(),
        expires_at: pairing.expires_at,
        local_confirmed: pairing.local_confirmed,
        remote_confirmed: pairing.remote_confirmed,
        sas_code: pairing.sas_code.clone(),
    }
}
fn trusted_view(peer: &TrustedPeer) -> TrustedDeviceView {
    TrustedDeviceView {
        device_id: peer.device_id.clone(),
        name: peer.remote_name.clone(),
        alias: peer.local_alias.clone(),
        paired_at: peer.paired_at,
        last_seen_at: peer.last_seen_at,
        certificate_fingerprint_short: peer.certificate_fingerprint.chars().take(16).collect(),
        auto_send: peer.auto_send,
        endpoint: peer.endpoint.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_store::InMemorySecretStore;
    use crate::state::{AppState, EnqueuePathsRequest};
    use std::{fs, sync::Arc};

    fn repository() -> (tempfile::TempDir, Arc<SettingsRepository>) {
        let directory = tempfile::tempdir().unwrap();
        let repository = Arc::new(
            SettingsRepository::open(directory.path().join("fileporter.sqlite3")).unwrap(),
        );
        (directory, repository)
    }
    fn remote_key(byte: u8) -> Vec<u8> {
        DeviceIdentity::from_secret_bytes([byte; 32])
            .public_identity()
            .public_key
            .to_vec()
    }
    fn request(coordinator: &PairingCoordinator) -> PendingPairingView {
        coordinator
            .request(
                "Remote Mac".into(),
                remote_key(9),
                format!("blake3:{}", "a1".repeat(32)),
            )
            .unwrap()
    }
    fn authenticated_request(coordinator: &PairingCoordinator) -> PendingPairingView {
        let remote = DeviceIdentity::from_secret_bytes([9; 32]).public_identity();
        coordinator
            .request_authenticated(
                "Remote Mac".into(),
                &fileporter_network::TrustedPeerPin {
                    device_id: remote.device_id(),
                    public_key: remote.public_key,
                    certificate_fingerprint: [7; 32],
                },
                "123 456".into(),
            )
            .unwrap()
    }

    #[test]
    fn secret_store_first_run_and_reload_keep_identity_and_tls_out_of_sqlite() {
        let (_directory, repository) = repository();
        let secrets = std::sync::Arc::new(InMemorySecretStore::default());
        let first = PairingCoordinator::open_with_secret_store(repository.clone(), secrets.clone())
            .unwrap();
        let first_id = first.snapshot().unwrap().local_device_id;
        let first_fingerprint = first.local_certificate().fingerprint();
        drop(first);
        let second =
            PairingCoordinator::open_with_secret_store(repository.clone(), secrets).unwrap();
        assert_eq!(second.snapshot().unwrap().local_device_id, first_id);
        assert_eq!(second.local_certificate().fingerprint(), first_fingerprint);
        assert!(repository.legacy_identity_secret().unwrap().is_none());
        assert!(repository.legacy_tls_credentials().unwrap().is_none());
    }

    #[test]
    fn v9_private_material_migrates_only_after_binding_and_os_store_write() {
        let (_directory, repository) = repository();
        let identity = DeviceIdentity::from_secret_bytes([4; 32]);
        let certificate = LocalCertificate::generate(&identity).unwrap();
        let secret = identity.export_secret_bytes();
        repository
            .save_legacy_identity_secret_for_test(&secret, 1)
            .unwrap();
        let (certificate_der, private_key_der) = certificate.persisted_der();
        repository
            .save_legacy_tls_credentials_for_test(&certificate_der, &private_key_der, 1)
            .unwrap();
        let secrets = std::sync::Arc::new(InMemorySecretStore::default());
        let coordinator =
            PairingCoordinator::open_with_secret_store(repository.clone(), secrets).unwrap();
        assert_eq!(
            coordinator.snapshot().unwrap().local_device_id,
            identity.public_identity().device_id()
        );
        assert_eq!(
            coordinator.local_certificate().fingerprint(),
            certificate.fingerprint()
        );
        assert!(repository.legacy_identity_secret().unwrap().is_none());
        assert!(repository.legacy_tls_credentials().unwrap().is_none());
    }

    #[test]
    fn failed_secret_store_migration_keeps_legacy_material_for_retry() {
        let (_directory, repository) = repository();
        let identity = DeviceIdentity::from_secret_bytes([5; 32]);
        let secret = identity.export_secret_bytes();
        repository
            .save_legacy_identity_secret_for_test(&secret, 1)
            .unwrap();
        let certificate = LocalCertificate::generate(&identity).unwrap();
        let (certificate_der, private_key_der) = certificate.persisted_der();
        repository
            .save_legacy_tls_credentials_for_test(&certificate_der, &private_key_der, 1)
            .unwrap();
        let secrets = std::sync::Arc::new(InMemorySecretStore::default());
        secrets.fail_writes();
        assert!(PairingCoordinator::open_with_secret_store(repository.clone(), secrets).is_err());
        assert!(repository.legacy_identity_secret().unwrap().is_some());
        assert!(repository.legacy_tls_credentials().unwrap().is_some());
    }

    #[test]
    fn identity_reloads_without_exposing_secret_in_snapshots() {
        let (_directory, repository) = repository();
        let secrets = std::sync::Arc::new(InMemorySecretStore::default());
        let first_coordinator =
            PairingCoordinator::open_with_secret_store(repository.clone(), secrets.clone())
                .unwrap();
        let first_certificate = first_coordinator.local_certificate().fingerprint();
        let first = first_coordinator.snapshot().unwrap();
        drop(first_coordinator);
        let second_coordinator =
            PairingCoordinator::open_with_secret_store(repository, secrets).unwrap();
        let second_certificate = second_coordinator.local_certificate().fingerprint();
        let second = second_coordinator.snapshot().unwrap();
        assert_eq!(first.local_device_id, second.local_device_id);
        assert_eq!(first_certificate, second_certificate);
        let json = serde_json::to_string(&first).unwrap();
        assert!(!json.contains("secret"));
        assert!(!json.contains("privateKey"));
        assert!(!json.contains("certificateDer"));
        assert!(!json.contains("publicKey"));
    }

    #[test]
    fn authenticated_pairings_persist_a_matching_normalized_sas_without_proof_material() {
        let (_left_directory, left_repository) = repository();
        let (_right_directory, right_repository) = repository();
        let left = PairingCoordinator::open_with_secret_store(
            left_repository.clone(),
            std::sync::Arc::new(InMemorySecretStore::default()),
        )
        .unwrap();
        let right = PairingCoordinator::open_with_secret_store(
            right_repository.clone(),
            std::sync::Arc::new(InMemorySecretStore::default()),
        )
        .unwrap();
        let left_pin = fileporter_network::TrustedPeerPin {
            device_id: right.identity.public_identity().device_id(),
            public_key: right.identity.public_identity().public_key,
            certificate_fingerprint: right.local_certificate().fingerprint(),
        };
        let right_pin = fileporter_network::TrustedPeerPin {
            device_id: left.identity.public_identity().device_id(),
            public_key: left.identity.public_identity().public_key,
            certificate_fingerprint: left.local_certificate().fingerprint(),
        };
        let left_pending = left
            .request_authenticated("Right".into(), &left_pin, "042 007".into())
            .unwrap();
        let right_pending = right
            .request_authenticated("Left".into(), &right_pin, "042 007".into())
            .unwrap();
        assert_eq!(left_pending.sas_code, Some("042 007".into()));
        assert_eq!(left_pending.sas_code, right_pending.sas_code);
        drop(left);
        // Reload through the same test store to prove snapshots remain redacted.
        let reloaded = PairingCoordinator::open_with_secret_store(
            left_repository,
            std::sync::Arc::new(InMemorySecretStore::default()),
        )
        .unwrap();
        let json = serde_json::to_string(&reloaded.snapshot().unwrap()).unwrap();
        assert!(json.contains("\"sasCode\":\"042 007\""));
        assert!(!json.contains("publicKey"));
        assert!(!json.contains("signature"));
        assert!(!json.contains("transcript"));
        assert!(!json.contains("certificateDer"));
    }

    #[test]
    fn pairing_without_authenticated_proof_cannot_be_confirmed() {
        let (_directory, repository) = repository();
        let coordinator = PairingCoordinator::open_with_secret_store(
            repository.clone(),
            std::sync::Arc::new(InMemorySecretStore::default()),
        )
        .unwrap();
        let pending = request(&coordinator);
        assert_eq!(pending.sas_code, None);
        assert!(matches!(
            coordinator.confirm(&pending.id),
            Err(AppError::Validation {
                code: "invalid_pairing",
                ..
            })
        ));
        assert!(!repository.pending_pairings().unwrap().is_empty());
        assert!(repository.active_trusted_peers().unwrap().is_empty());
    }

    #[test]
    fn expired_and_rejected_requests_never_create_peers() {
        let (_directory, repository) = repository();
        repository
            .save_pending_pairing(&PendingPairing {
                id: "expired".into(),
                device_id: "expired-device".into(),
                public_key: remote_key(8),
                certificate_fingerprint: "sha256:expired".into(),
                remote_name: "Expired".into(),
                created_at: 1,
                expires_at: 1,
                local_confirmed: false,
                remote_confirmed: false,
                sas_code: None,
            })
            .unwrap();
        let coordinator = PairingCoordinator::open_with_secret_store(
            repository.clone(),
            std::sync::Arc::new(InMemorySecretStore::default()),
        )
        .unwrap();
        assert!(coordinator.snapshot().unwrap().pending_pairings.is_empty());
        let pending = request(&coordinator);
        coordinator.reject(&pending.id).unwrap();
        assert!(matches!(
            coordinator.confirm(&pending.id),
            Err(AppError::Validation {
                code: "pairing_not_found",
                ..
            })
        ));
        assert!(repository.active_trusted_peers().unwrap().is_empty());
    }

    #[test]
    fn confirmation_transaction_refuses_an_expired_persisted_record() {
        let (_directory, repository) = repository();
        let public_key = remote_key(7);
        repository
            .save_pending_pairing(&PendingPairing {
                id: "expired-transaction".into(),
                device_id: "peer-7".into(),
                public_key: public_key.clone(),
                certificate_fingerprint: "sha256:expired".into(),
                remote_name: "Expired".into(),
                created_at: 1,
                expires_at: 2,
                local_confirmed: false,
                remote_confirmed: false,
                sas_code: None,
            })
            .unwrap();
        let peer = TrustedPeer {
            device_id: "peer-7".into(),
            public_key,
            certificate_fingerprint: "sha256:expired".into(),
            remote_name: "Expired".into(),
            local_alias: None,
            paired_at: 2,
            last_seen_at: None,
            auto_send: false,
            revoked_at: None,
            endpoint: None,
        };
        assert!(matches!(
            repository.commit_confirmed_pairing("expired-transaction", &peer, 2),
            Err(AppError::Validation {
                code: "pairing_expired",
                ..
            })
        ));
        assert!(repository.pending_pairings().unwrap().is_empty());
        assert!(repository.active_trusted_peers().unwrap().is_empty());
    }

    #[test]
    fn confirmation_commits_peer_and_forget_revokes_enqueue_access() {
        let (directory, repository) = repository();
        let coordinator = PairingCoordinator::open_with_secret_store(
            repository.clone(),
            std::sync::Arc::new(InMemorySecretStore::default()),
        )
        .unwrap();
        let pending = authenticated_request(&coordinator);
        let waiting = coordinator.confirm(&pending.id).unwrap();
        assert!(waiting.trusted_device.is_none());
        let trusted = coordinator.confirm_remote(&pending.id).unwrap().unwrap();
        assert_eq!(repository.active_trusted_peers().unwrap().len(), 1);
        assert!(repository.pending_pairings().unwrap().is_empty());
        coordinator.forget(&trusted.device_id).unwrap();
        assert!(repository.active_trusted_peers().unwrap().is_empty());
        let source = directory.path().join("notes.txt");
        fs::write(&source, b"notes").unwrap();
        drop(coordinator);
        let state = AppState::new(Arc::try_unwrap(repository).ok().unwrap());
        assert!(matches!(
            state.queue_batch(EnqueuePathsRequest {
                paths: vec![source.display().to_string()],
                target_device_ids: vec![trusted.device_id],
                queue_offline: false
            }),
            Err(AppError::Validation {
                code: "unknown_recipient",
                ..
            })
        ));
    }
}
