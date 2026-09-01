//! Application-facing listener lifecycle for Fileporter's runtime core.
//!
//! It owns a cancellable TCP listener foundation only. It does not advertise
//! peers, discover devices, or authorize transfers.

use std::{
    collections::HashMap,
    io::{self, Read},
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU8, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

#[cfg(test)]
static MAX_SOURCE_READ_BUFFER_FOR_TEST: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn reset_source_read_max_for_test() {
    MAX_SOURCE_READ_BUFFER_FOR_TEST.store(0, Ordering::Release);
}

#[cfg(test)]
pub(crate) fn source_read_max_for_test() -> usize {
    MAX_SOURCE_READ_BUFFER_FOR_TEST.load(Ordering::Acquire)
}

use fileporter_identity::{PairingParticipant, PairingRole, PairingTranscript};
use fileporter_network::{
    accept_identity_authenticated_stream, accept_pairing_authenticated,
    accept_pairing_stream_authenticated, connect_authenticated, pairing_server_config,
    LocalCertificate, TrustMode, TrustedPeerPin,
};
use fileporter_protocol::{
    BatchComplete, BatchReceipt, Chunk, ControlMessage, EntryComplete, EntryKind, EntryVerified,
    Frame, ManifestEntry, ManifestPage, OfferAccept, OfferStart, TopLevelItem,
};
use rand_core::{OsRng, RngCore};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::JoinHandle,
    time::{timeout_at, Duration, Instant},
};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

const DORMANT: u8 = 0;
const SHUT_DOWN: u8 = 1;
const MAX_PAIRING_SESSIONS: usize = 8;
const MAX_PAIRINGS_PER_SOURCE: usize = 2;
const PAIRING_QUEUE: usize = 16;

/// The lifecycle states the engine can honestly report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineLifecycle {
    Dormant,
    ShutDown,
}

/// Observable listener state. Neither field implies peer discovery or transfer
/// authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenerStatus {
    pub listening: bool,
    pub receiving: bool,
    pub bound_endpoint: Option<SocketAddr>,
}

#[derive(Debug)]
pub enum ListenerError {
    InvalidAddress,
    ShuttingDown,
    AlreadyListening(SocketAddr),
    Io(io::Error),
}
impl std::fmt::Display for ListenerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAddress => {
                write!(f, "manual endpoints must use a loopback or private address")
            }
            Self::ShuttingDown => write!(f, "the engine is shutting down"),
            Self::AlreadyListening(address) => write!(f, "already listening on {address}"),
            Self::Io(_) => write!(f, "listener I/O failed"),
        }
    }
}
impl std::error::Error for ListenerError {}
impl From<io::Error> for ListenerError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

struct ListenerSession {
    address: SocketAddr,
    cancellation: CancellationToken,
    task: JoinHandle<()>,
    connection_tasks: TaskTracker,
}

/// Long-lived owner for listener work. Its cancellation token gives shutdown a
/// deterministic boundary even when the UI has been hidden.
pub struct Engine {
    lifecycle: AtomicU8,
    listener: Mutex<Option<ListenerSession>>,
    active_connections: Arc<AtomicUsize>,
    pairing: Arc<PairingService>,
    receiver: Option<Arc<ReceiverService>>,
}

struct ReceiverService {
    settings: Arc<crate::persistence::SettingsRepository>,
    pairing: Arc<crate::identity::PairingCoordinator>,
    events: crate::state_events::StateEvents,
}

/// Runtime-only pairing ownership. The coordinator owns durable state; this
/// service owns authenticated live streams and bounds untrusted admission.
pub struct PairingService {
    coordinator: Arc<crate::identity::PairingCoordinator>,
    active: Arc<Mutex<HashMap<String, ActivePairing>>>,
    source_counts: Arc<Mutex<HashMap<IpAddr, usize>>>,
    queued: AtomicUsize,
    cancellation: CancellationToken,
}
struct ActivePairing {
    session_id: uuid::Uuid,
    commands: mpsc::Sender<ControlMessage>,
}

/// All data required to send one already-queued regular file to one explicitly
/// chosen trusted endpoint. The application resolves the endpoint from a
/// durable peer record before constructing this transport-only request.
pub struct LoopbackFileTransfer {
    pub endpoint: SocketAddr,
    pub local_certificate: LocalCertificate,
    pub trusted_peer: TrustedPeerPin,
    pub batch_id: uuid::Uuid,
    pub entry_id: uuid::Uuid,
    pub source: std::path::PathBuf,
    pub display_name: String,
    /// Last receiver-durable offset.  It is obtained from a ChunkAck and is
    /// therefore safe to use as the first byte on a fresh authenticated stream.
    pub resume_offset: u64,
    pub cancellation: CancellationToken,
}

/// One manifest entry in a durable batch.  `components` are receiver-relative,
/// never local absolute paths; the local path is used only by the sender.
#[derive(Clone)]
pub struct LoopbackBatchEntry {
    pub entry_id: uuid::Uuid,
    pub parent_entry_id: Option<uuid::Uuid>,
    pub kind: EntryKind,
    pub components: Vec<String>,
    pub source: std::path::PathBuf,
    pub size: u64,
    pub mtime: Option<String>,
    pub resume_offset: u64,
}

/// Authenticated transport request for a complete manifest. Entries must be
/// parent-before-child and bounded by the protocol/transfer manifest limits.
pub struct LoopbackBatchTransfer {
    pub endpoint: SocketAddr,
    pub local_certificate: LocalCertificate,
    pub trusted_peer: TrustedPeerPin,
    pub batch_id: uuid::Uuid,
    pub entries: Vec<LoopbackBatchEntry>,
    pub cancellation: CancellationToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferProgress {
    pub acknowledged_bytes: u64,
    pub total_bytes: u64,
}

/// Result of an authenticated, pairing-only exchange.  It intentionally has
/// no transfer capability; callers may persist its pin only after both local
/// and remote explicit confirmations have been observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingExchange {
    pub peer: TrustedPeerPin,
    pub sas: String,
    pub session_id: uuid::Uuid,
    pub remote_name: String,
}

#[allow(dead_code)] // exercised by the pairing transport tests; retained as a narrow transport primitive.
pub async fn initiate_pairing(
    endpoint: SocketAddr,
    local: &LocalCertificate,
    device_name: &str,
) -> Result<PairingExchange, ListenerError> {
    if endpoint.port() == 0 || !is_loopback_or_private(endpoint.ip()) || device_name.len() > 128 {
        return Err(ListenerError::InvalidAddress);
    }
    let (mut stream, authenticated) = connect_authenticated(endpoint, local, TrustMode::Pairing)
        .await
        .map_err(network_io)?;
    if authenticated.authorization != fileporter_network::SessionAuthorization::PairingOnly {
        return Err(pairing_io("pairing authorization required"));
    }
    exchange_pairing(
        &mut stream,
        local,
        authenticated.peer,
        device_name,
        PairingRole::Initiator,
    )
    .await
}

#[allow(dead_code)] // exercised by the pairing transport tests; retained as a narrow transport primitive.
pub async fn accept_pairing(
    listener: &TcpListener,
    local: &LocalCertificate,
    device_name: &str,
) -> Result<PairingExchange, ListenerError> {
    let acceptor = TlsAcceptor::from(pairing_server_config(local).map_err(network_io)?);
    let (mut stream, authenticated) = accept_pairing_authenticated(listener, acceptor, local)
        .await
        .map_err(network_io)?;
    if authenticated.authorization != fileporter_network::SessionAuthorization::PairingOnly {
        return Err(pairing_io("pairing authorization required"));
    }
    exchange_pairing(
        &mut stream,
        local,
        authenticated.peer,
        device_name,
        PairingRole::Responder,
    )
    .await
}

async fn exchange_pairing<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    stream: &mut S,
    local: &LocalCertificate,
    peer: fileporter_network::PeerBinding,
    device_name: &str,
    role: PairingRole,
) -> Result<PairingExchange, ListenerError> {
    let mut session_id = uuid::Uuid::new_v4();
    let mut nonce = [0u8; 32];
    OsRng.fill_bytes(&mut nonce);
    let request = ControlMessage::PairRequest(fileporter_protocol::PairRequest {
        session_id,
        device_name: device_name.to_owned(),
        nonce: hex::encode(nonce),
        expires_at: "120".into(),
    });
    let remote = match role {
        PairingRole::Initiator => {
            send_control_frame(stream, request).await?;
            receive_control_frame(stream).await?
        }
        PairingRole::Responder => {
            let remote = receive_control_frame(stream).await?;
            let ControlMessage::PairRequest(ref remote_request) = remote else {
                return Err(pairing_io("only pairing request frames are accepted"));
            };
            session_id = remote_request.session_id;
            let response = ControlMessage::PairRequest(fileporter_protocol::PairRequest {
                session_id,
                device_name: device_name.to_owned(),
                nonce: hex::encode(nonce),
                expires_at: "120".into(),
            });
            send_control_frame(stream, response).await?;
            remote
        }
    };
    let fileporter_protocol::ControlMessage::PairRequest(remote) = remote else {
        return Err(pairing_io("only pairing request frames are accepted"));
    };
    if remote.device_name.trim().is_empty()
        || remote.device_name.len() > 128
        || remote.expires_at != "120"
    {
        return Err(pairing_io("invalid pairing request"));
    }
    let remote_nonce: [u8; 32] = hex::decode(&remote.nonce)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| pairing_io("invalid pairing nonce"))?;
    let (initiator, responder) = match role {
        PairingRole::Initiator => (
            PairingParticipant::new(
                PairingRole::Initiator,
                local.binding().identity.clone(),
                local.fingerprint(),
                nonce,
            ),
            PairingParticipant::new(
                PairingRole::Responder,
                peer.identity.clone(),
                peer.certificate_fingerprint,
                remote_nonce,
            ),
        ),
        PairingRole::Responder => (
            PairingParticipant::new(
                PairingRole::Initiator,
                peer.identity.clone(),
                peer.certificate_fingerprint,
                remote_nonce,
            ),
            PairingParticipant::new(
                PairingRole::Responder,
                local.binding().identity.clone(),
                local.fingerprint(),
                nonce,
            ),
        ),
    };
    let transcript = PairingTranscript::new(initiator, responder)
        .map_err(|_| pairing_io("invalid transcript"))?;
    let local_identity = local.identity_for_pairing();
    let proof = local_identity.sign_pairing_transcript(&transcript);
    let proof_message = ControlMessage::PairProof(fileporter_protocol::PairProof {
        session_id: remote.session_id,
        transcript_hash: blake3::hash(&transcript.canonical_bytes())
            .to_hex()
            .to_string(),
        signature: hex::encode(proof.signature),
    });
    let remote_proof = match role {
        PairingRole::Initiator => {
            send_control_frame(stream, proof_message).await?;
            receive_control_frame(stream).await?
        }
        PairingRole::Responder => {
            let remote = receive_control_frame(stream).await?;
            send_control_frame(stream, proof_message).await?;
            remote
        }
    };
    let fileporter_protocol::ControlMessage::PairProof(remote_proof) = remote_proof else {
        return Err(pairing_io("only pairing proof frames are accepted"));
    };
    if remote.session_id != session_id
        || remote_proof.session_id != session_id
        || remote_proof.transcript_hash
            != blake3::hash(&transcript.canonical_bytes())
                .to_hex()
                .to_string()
    {
        return Err(pairing_io("pairing proof replay or transcript mismatch"));
    }
    let signature: [u8; 64] = hex::decode(remote_proof.signature)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| pairing_io("invalid pairing proof"))?;
    let proofs = vec![
        proof,
        fileporter_identity::PairingProof {
            public_key: peer.identity.public_key,
            signature,
        },
    ];
    let sas = transcript
        .sas(&proofs)
        .map_err(|_| pairing_io("pairing proof verification failed"))?;
    Ok(PairingExchange {
        peer: TrustedPeerPin::from_binding(&peer),
        sas: sas.formatted(),
        session_id,
        remote_name: remote.device_name,
    })
}

fn pairing_io(message: &'static str) -> ListenerError {
    ListenerError::Io(io::Error::new(io::ErrorKind::PermissionDenied, message))
}

impl PairingService {
    async fn accept_authenticated_tls<S>(
        &self,
        mut tls: S,
        peer: fileporter_network::PeerBinding,
        source: IpAddr,
        device_name: String,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let source = normalized_source(source);
        if self.queued.fetch_add(1, Ordering::AcqRel) >= PAIRING_QUEUE
            || self.active.lock().expect("pairing mutex poisoned").len() >= MAX_PAIRING_SESSIONS
        {
            self.queued.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        let exchange = exchange_pairing(
            &mut tls,
            &self.coordinator.local_certificate(),
            peer,
            &device_name,
            PairingRole::Responder,
        )
        .await;
        if let Ok(exchange) = exchange {
            if let Ok(pending) = self.coordinator.request_authenticated(
                exchange.remote_name,
                &exchange.peer,
                exchange.sas,
            ) {
                self.register(pending.id.clone(), exchange.session_id, tls);
                self.confirm_automatically(&pending.id);
            }
        }
        let _ = source;
        self.queued.fetch_sub(1, Ordering::AcqRel);
    }
    async fn start_outgoing(
        &self,
        endpoint: SocketAddr,
        device_name: String,
        expected_peer: Option<(&str, &str)>,
    ) -> Result<crate::identity::PendingPairingView, ListenerError> {
        if endpoint.port() == 0
            || !is_loopback_or_private(endpoint.ip())
            || self.active.lock().expect("pairing mutex poisoned").len() >= MAX_PAIRING_SESSIONS
        {
            return Err(pairing_io("pairing capacity reached"));
        }
        let local = self.coordinator.local_certificate();
        let (mut stream, authenticated) =
            connect_authenticated(endpoint, &local, TrustMode::Pairing)
                .await
                .map_err(network_io)?;
        let exchange = exchange_pairing(
            &mut stream,
            &local,
            authenticated.peer,
            &device_name,
            PairingRole::Initiator,
        )
        .await?;
        if let Some((expected_device_id, expected_fingerprint)) = expected_peer {
            let actual_fingerprint = format!(
                "blake3:{}",
                hex::encode(exchange.peer.certificate_fingerprint)
            );
            if exchange.peer.device_id != expected_device_id
                || !actual_fingerprint.eq_ignore_ascii_case(expected_fingerprint)
            {
                return Err(pairing_io("discovered identity did not match endpoint"));
            }
        }
        let pending = self
            .coordinator
            .request_authenticated(exchange.remote_name, &exchange.peer, exchange.sas)
            .map_err(|_| pairing_io("could not create pairing"))?;
        self.register(pending.id.clone(), exchange.session_id, stream);
        self.confirm_automatically(&pending.id);
        Ok(pending)
    }

    fn confirm_automatically(&self, id: &str) {
        if !self.coordinator.automatic_device_trust_enabled() {
            return;
        }
        let Ok(session_id) = self.session_id(id) else {
            return;
        };
        if self.coordinator.confirm(id).is_ok() {
            self.send(
                id,
                ControlMessage::PairConfirmed(fileporter_protocol::PairConfirmed { session_id }),
            );
        }
    }
    fn session_id(&self, id: &str) -> Result<uuid::Uuid, crate::error::AppError> {
        self.active
            .lock()
            .expect("pairing mutex poisoned")
            .get(id)
            .map(|p| p.session_id)
            .ok_or(crate::error::AppError::Validation {
                code: "pairing_not_found",
                message: "That pairing request is no longer available.",
                field: Some("pairingId"),
            })
    }
    fn send(&self, id: &str, message: ControlMessage) {
        if let Some(pairing) = self.active.lock().expect("pairing mutex poisoned").get(id) {
            let _ = pairing.commands.try_send(message);
        }
    }
    fn register<S>(&self, id: String, session_id: uuid::Uuid, stream: S)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (tx, mut rx) = mpsc::channel(2);
        self.active.lock().expect("pairing mutex poisoned").insert(
            id.clone(),
            ActivePairing {
                session_id,
                commands: tx,
            },
        );
        let coordinator = self.coordinator.clone();
        let active = self.active.clone();
        let cancellation = self.cancellation.clone();
        // The task is the persistent authenticated control listener. It accepts
        // exactly the matching confirmation/rejection and never transfer frames.
        tokio::spawn(async move {
            let mut stream = stream;
            let deadline = Instant::now() + Duration::from_secs(120);
            let mut local_confirmation_sent = false;
            let mut remote_confirmed = false;
            loop {
                tokio::select! {
                    _ = cancellation.cancelled() => { let _ = coordinator.reject(&id); break; }
                    outbound = rx.recv() => match outbound {
                        Some(message) => {
                            let confirms = matches!(message, ControlMessage::PairConfirmed(_));
                            if send_control_frame(&mut stream, message).await.is_err() { break; }
                            if confirms {
                                local_confirmation_sent = true;
                                if remote_confirmed { break; }
                            }
                        },
                        None => break
                    },
                    inbound = timeout_at(deadline, receive_control_frame(&mut stream)) => match inbound {
                        Ok(Ok(ControlMessage::PairConfirmed(value))) if value.session_id == session_id => {
                            if coordinator.confirm_remote(&id).is_err() { break; }
                            remote_confirmed = true;
                            if local_confirmation_sent { break; }
                        }
                        Ok(Ok(ControlMessage::PairRejected(value))) if value.session_id == session_id => { let _ = coordinator.reject(&id); break; }
                        _ => { let _ = coordinator.reject(&id); break; }
                    }
                }
            }
            active.lock().expect("pairing mutex poisoned").remove(&id);
        });
    }
    async fn accept_incoming(&self, stream: TcpStream, source: IpAddr, device_name: String) {
        let source = normalized_source(source);
        if self.queued.fetch_add(1, Ordering::AcqRel) >= PAIRING_QUEUE
            || self.active.lock().expect("pairing mutex poisoned").len() >= MAX_PAIRING_SESSIONS
        {
            self.queued.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        {
            let mut sources = self.source_counts.lock().expect("pairing mutex poisoned");
            let count = sources.entry(source).or_default();
            if *count >= MAX_PAIRINGS_PER_SOURCE {
                self.queued.fetch_sub(1, Ordering::AcqRel);
                return;
            }
            *count += 1;
        }
        let local = self.coordinator.local_certificate();
        let acceptor = match pairing_server_config(&local) {
            Ok(config) => TlsAcceptor::from(config),
            Err(_) => {
                self.release_source(source);
                self.queued.fetch_sub(1, Ordering::AcqRel);
                return;
            }
        };
        let result = async {
            let (mut tls, authenticated) =
                accept_pairing_stream_authenticated(stream, acceptor, &local)
                    .await
                    .map_err(network_io)?;
            let exchange = exchange_pairing(
                &mut tls,
                &local,
                authenticated.peer,
                &device_name,
                PairingRole::Responder,
            )
            .await?;
            let pending = self
                .coordinator
                .request_authenticated(exchange.remote_name, &exchange.peer, exchange.sas)
                .map_err(|_| pairing_io("could not create pairing"))?;
            self.register(pending.id.clone(), exchange.session_id, tls);
            self.confirm_automatically(&pending.id);
            Ok::<(), ListenerError>(())
        }
        .await;
        self.release_source(source);
        self.queued.fetch_sub(1, Ordering::AcqRel);
        let _ = result;
    }
    fn release_source(&self, source: IpAddr) {
        let mut sources = self.source_counts.lock().expect("pairing mutex poisoned");
        if let Some(count) = sources.get_mut(&source) {
            *count -= 1;
            if *count == 0 {
                sources.remove(&source);
            }
        }
    }
    fn shutdown(&self) {
        self.cancellation.cancel();
        self.active.lock().expect("pairing mutex poisoned").clear();
    }
}

impl ReceiverService {
    /// Reconciles only app-owned staging at process startup.  A database
    /// checkpoint is never trusted beyond the bytes still present on disk;
    /// completed files are adopted only when their recorded final path and
    /// verified BLAKE3 digest both prove the ownership relationship.
    fn reconcile_startup(&self) -> Result<(), crate::error::AppError> {
        self.reconcile_startup_at(unix_timestamp())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn reconcile_startup_at(&self, now: i64) -> Result<(), crate::error::AppError> {
        const ORPHAN_RETENTION_SECS: i64 = 30 * 24 * 60 * 60;
        let config = self.settings.load()?;
        let Some(receive) = config.receive_directory else {
            return Ok(());
        };
        let root = std::path::Path::new(&receive);
        let records = self.settings.all_batches()?;
        let incoming: std::collections::HashMap<_, _> = records
            .iter()
            .filter(|record| record.batch.direction == "incoming")
            .map(|record| (record.batch.id.clone(), record))
            .collect();

        for record in incoming
            .values()
            .filter(|record| record.batch.state != "completed")
        {
            let Ok(batch_id) = uuid::Uuid::parse_str(&record.batch.id) else {
                self.mark_recovery_failed(record, "recovery_invalid_batch")?;
                continue;
            };
            let Ok(area) = fileporter_transfer::StagingArea::open(root, batch_id) else {
                // A finalized path without a matching verified checkpoint is
                // deliberately not adopted: it might be a user's own file.
                if !self.adopt_proven_final_files(record)? {
                    self.mark_recovery_failed(record, "recovery_staging_missing")?;
                }
                continue;
            };
            let target = match record.targets.first() {
                Some(value) => value,
                None => {
                    let _ = area.cleanup_owned();
                    self.mark_recovery_failed(record, "recovery_target_missing")?;
                    continue;
                }
            };
            let mut invalid = false;
            for item in record.items.iter().filter(|item| item.kind == "file") {
                let Some(components) = item_components(item, &record.items) else {
                    invalid = true;
                    break;
                };
                let staged = match area.relative_path(&components) {
                    Ok(path) => path,
                    Err(_) => {
                        invalid = true;
                        break;
                    }
                };
                let checkpoint = self.settings.checkpoint(&target.id, &item.id)?;
                let disk_len = std::fs::metadata(&staged)
                    .ok()
                    .filter(|meta| meta.is_file())
                    .map(|meta| meta.len());
                let Some(disk_len) = disk_len else {
                    invalid = true;
                    break;
                };
                if disk_len > item.size.max(0) as u64 {
                    invalid = true;
                    break;
                }
                let durable = checkpoint
                    .as_ref()
                    .map(|value| value.durable_offset.max(0) as u64)
                    .unwrap_or(0);
                if checkpoint
                    .as_ref()
                    .and_then(|value| value.verified_hash.as_ref())
                    .is_some()
                {
                    let expected = checkpoint.as_ref().unwrap().verified_hash.as_ref().unwrap();
                    if disk_len != item.size.max(0) as u64 || !path_hash_matches(&staged, expected)
                    {
                        invalid = true;
                        break;
                    }
                } else if durable != disk_len {
                    // Disk is authoritative.  If it has a suffix not recorded
                    // in SQLite, remove that unacknowledged suffix; if SQLite
                    // is ahead, lower it to the fsynced file length.
                    if disk_len > durable {
                        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&staged) {
                            if file.set_len(durable).is_err() {
                                invalid = true;
                                break;
                            }
                        } else {
                            invalid = true;
                            break;
                        }
                    }
                    self.settings
                        .save_checkpoint(&crate::persistence::Checkpoint {
                            target_id: target.id.clone(),
                            item_id: item.id.clone(),
                            durable_offset: durable.min(disk_len) as i64,
                            verified_hash: None,
                            updated_at: now,
                        })?;
                }
            }
            if invalid {
                let _ = area.cleanup_owned();
                self.mark_recovery_failed(record, "recovery_staging_corrupt")?;
            }
        }
        // Unreferenced UUID staging roots are app-owned.  Keep recent ones so
        // an interrupted process can restart, but prune old roots only.
        for path in fileporter_transfer::enumerate_abandoned_staging(root).unwrap_or_default() {
            let id = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| uuid::Uuid::parse_str(name).ok());
            let old = path
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .is_some_and(|time| {
                    now.saturating_sub(time.as_secs() as i64) >= ORPHAN_RETENTION_SECS
                });
            if let Some(id) = id.filter(|id| !incoming.contains_key(&id.to_string())) {
                if old {
                    if let Ok(area) = fileporter_transfer::StagingArea::open(root, id) {
                        let _ = area.cleanup_owned();
                    }
                }
            }
        }
        Ok(())
    }

    fn adopt_proven_final_files(
        &self,
        record: &crate::persistence::PersistedBatch,
    ) -> Result<bool, crate::error::AppError> {
        let Some(target) = record.targets.first() else {
            return Ok(false);
        };
        if record.items.iter().any(|item| item.kind != "file") {
            return Ok(false);
        }
        for item in &record.items {
            let Some(destination) = item.destination_path_local.as_ref() else {
                return Ok(false);
            };
            let Some(checkpoint) = self.settings.checkpoint(&target.id, &item.id)? else {
                return Ok(false);
            };
            let Some(hash) = checkpoint.verified_hash else {
                return Ok(false);
            };
            if checkpoint.durable_offset != item.size
                || !path_hash_matches(std::path::Path::new(destination), &hash)
            {
                return Ok(false);
            }
        }
        for item in &record.items {
            let mut value = item.clone();
            value.state = "completed".into();
            self.settings.save_item(&value)?;
        }
        let mut batch = record.batch.clone();
        batch.state = "completed".into();
        batch.completed_at = Some(unix_timestamp());
        self.settings.save_batch(&batch)?;
        let mut target = target.clone();
        target.state = "completed".into();
        target.error_code = None;
        self.settings.save_batch_target(&target)?;
        Ok(true)
    }

    fn mark_recovery_failed(
        &self,
        record: &crate::persistence::PersistedBatch,
        code: &str,
    ) -> Result<(), crate::error::AppError> {
        let mut batch = record.batch.clone();
        batch.state = "failed".into();
        batch.completed_at = Some(unix_timestamp());
        self.settings.save_batch(&batch)?;
        for target in &record.targets {
            let mut value = target.clone();
            value.state = "failed".into();
            value.error_code = Some(code.into());
            self.settings.save_batch_target(&value)?;
        }
        for item in &record.items {
            let mut value = item.clone();
            value.state = "failed".into();
            self.settings.save_item(&value)?;
        }
        Ok(())
    }
    async fn accept_or_pair(
        &self,
        stream: TcpStream,
        pairing_service: Arc<PairingService>,
        device_name: String,
        cancellation: CancellationToken,
    ) {
        let local = self.pairing.local_certificate();
        let acceptor = match pairing_server_config(&local) {
            Ok(v) => TlsAcceptor::from(v),
            Err(_) => return,
        };
        let source = stream
            .peer_addr()
            .map(|v| v.ip())
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
        let Ok((tls, authenticated)) =
            accept_identity_authenticated_stream(stream, acceptor, &local).await
        else {
            return;
        };
        let pin = fileporter_network::TrustedPeerPin::from_binding(&authenticated.peer);
        let trusted = self
            .settings
            .trusted_peer(&pin.device_id)
            .ok()
            .flatten()
            .filter(|peer| peer.revoked_at.is_none())
            .and_then(|peer| trusted_pin_from_record(&peer).ok())
            .is_some_and(|stored| stored == pin);
        if trusted {
            let _ = self.receive_batch(tls, pin, cancellation).await;
        } else {
            // This authenticated-but-unpinned session is pairing-only.  A
            // transfer offer on it is rejected by the pairing state machine.
            pairing_service
                .accept_authenticated_tls(tls, authenticated.peer, source, device_name)
                .await;
        }
    }

    /// Manifest receiver: validates the complete parent-before-child graph,
    /// stages every entry beneath the batch UUID, and finalizes only verified
    /// top-level entries without ever merging with an existing destination.
    async fn receive_batch<S>(
        &self,
        mut stream: S,
        peer: fileporter_network::TrustedPeerPin,
        cancellation: CancellationToken,
    ) -> Result<(), ListenerError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        use std::collections::{HashMap, HashSet};
        let config = self
            .settings
            .load()
            .map_err(|_| receive_io("settings unavailable"))?;
        if !config.receiving_enabled {
            return Err(receive_io("receiving disabled"));
        }
        let receive = config
            .receive_directory
            .ok_or_else(|| receive_io("receive root unavailable"))?;
        let root = std::path::Path::new(&receive);
        let offer = match receive_control_or_cancel(&mut stream, &cancellation).await? {
            ControlMessage::OfferStart(v) => v,
            _ => return Err(receive_io("transfer offer required")),
        };
        let page = match receive_control_or_cancel(&mut stream, &cancellation).await? {
            ControlMessage::ManifestPage(v) => v,
            _ => return Err(receive_io("manifest required")),
        };
        if offer.items.is_empty()
            || offer.total_entries == 0
            || offer.total_entries as usize > fileporter_transfer::MAX_ENTRIES
            || offer.total_bytes > i64::MAX as u64
            || page.batch_id != offer.batch_id
            || !page.final_page
            || page.page != 0
            || page.entries.len() != offer.total_entries as usize
        {
            return Err(receive_io("invalid manifest"));
        }
        let mut known = HashSet::new();
        let mut parents: HashMap<uuid::Uuid, &ManifestEntry> = HashMap::new();
        let mut tops = Vec::new();
        let mut total = 0u64;
        for entry in &page.entries {
            fileporter_transfer::validate_receiver_components(&entry.components)
                .map_err(|_| receive_io("unsafe manifest path"))?;
            if !known.insert(entry.entry_id) {
                return Err(receive_io("duplicate manifest entry"));
            }
            if let Some(id) = entry.parent_entry_id {
                let Some(parent) = parents.get(&id) else {
                    return Err(receive_io("manifest parent order"));
                };
                if parent.kind != EntryKind::Directory
                    || entry.components.len() != parent.components.len() + 1
                    || entry.components[..entry.components.len() - 1] != parent.components[..]
                {
                    return Err(receive_io("invalid manifest hierarchy"));
                }
            } else {
                if entry.components.len() != 1 {
                    return Err(receive_io("top-level path invalid"));
                }
                tops.push(entry);
            }
            match entry.kind {
                EntryKind::File => {
                    total = total
                        .checked_add(entry.size)
                        .ok_or_else(|| receive_io("batch too large"))?
                }
                EntryKind::Directory if entry.size == 0 => {}
                _ => return Err(receive_io("invalid directory entry")),
            }
            parents.insert(entry.entry_id, entry);
        }
        if total != offer.total_bytes
            || tops.len() != offer.items.len()
            || tops.iter().zip(&offer.items).any(|(e, i)| {
                e.entry_id != i.entry_id
                    || e.components[0] != i.name
                    || e.kind != i.kind
                    || e.size != i.size
            })
            || !receive_space_available(root, total)
        {
            return Err(receive_io("offer does not match manifest"));
        }
        let bid = offer.batch_id.to_string();
        let existing = self
            .settings
            .all_batches()
            .map_err(|_| receive_io("batch load failed"))?
            .into_iter()
            .find(|v| v.batch.id == bid && v.batch.direction == "incoming");
        if existing
            .as_ref()
            .is_some_and(|v| v.batch.state == "completed")
        {
            return Err(receive_io("batch already finalized"));
        }
        let target = existing
            .as_ref()
            .and_then(|v| v.targets.first())
            .map(|v| v.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let area = if existing.is_some() {
            fileporter_transfer::StagingArea::open(root, offer.batch_id)
        } else {
            fileporter_transfer::StagingArea::create(root, offer.batch_id)
        }
        .map_err(|_| receive_io("unsafe staging area"))?;
        let now = unix_timestamp();
        self.settings
            .save_batch(&crate::persistence::Batch {
                id: bid.clone(),
                direction: "incoming".into(),
                state: "receiving".into(),
                created_at: now,
                completed_at: None,
                total_bytes: total as i64,
                total_entries: page.entries.len() as i64,
                warning_count: 0,
                wait_for_available: false,
            })
            .map_err(|_| receive_io("batch persistence failed"))?;
        self.settings
            .save_batch_target(&crate::persistence::BatchTarget {
                id: target.clone(),
                batch_id: bid.clone(),
                peer_device_id: peer.device_id.clone(),
                state: "receiving".into(),
                acknowledged_bytes: 0,
                error_code: None,
                retry_at: None,
                retry_count: 0,
                wait_for_available: false,
            })
            .map_err(|_| receive_io("target persistence failed"))?;
        let mut checkpoints = Vec::new();
        for e in &page.entries {
            let id = e.entry_id.to_string();
            self.settings
                .save_item(&crate::persistence::TransferItem {
                    id: id.clone(),
                    batch_id: bid.clone(),
                    parent_item_id: e.parent_entry_id.map(|v| v.to_string()),
                    kind: if e.kind == EntryKind::File {
                        "file"
                    } else {
                        "directory"
                    }
                    .into(),
                    display_name: e.components.last().unwrap().clone(),
                    source_path_local: None,
                    destination_path_local: None,
                    size: e.size as i64,
                    mtime: None,
                    state: "receiving".into(),
                    warning_json: None,
                })
                .map_err(|_| receive_io("item persistence failed"))?;
            if e.kind == EntryKind::Directory {
                area.create_directories(&e.components)
                    .map_err(|_| receive_io("directory staging failed"))?;
            }
            if let Some(c) = self
                .settings
                .incoming_checkpoint(&bid, &id)
                .map_err(|_| receive_io("checkpoint load failed"))?
            {
                if c.durable_offset > 0 {
                    checkpoints.push(fileporter_protocol::Checkpoint {
                        entry_id: e.entry_id,
                        durable_offset: c.durable_offset as u64,
                    });
                }
            }
        }
        send_control_frame(
            &mut stream,
            ControlMessage::OfferAccept(OfferAccept {
                batch_id: offer.batch_id,
                destination_generation: 0,
                resolved_top_level_names: tops.iter().map(|e| e.components[0].clone()).collect(),
                available_space_ok: true,
                checkpoints,
            }),
        )
        .await?;
        let mut integrity_failed = false;
        let work: Result<(), ListenerError> = async {
            for e in page.entries.iter().filter(|e| e.kind == EntryKind::File) {
                let id = e.entry_id.to_string();
                let prior = self
                    .settings
                    .incoming_checkpoint(&bid, &id)
                    .map_err(|_| receive_io("checkpoint load failed"))?;
                if let Some(prior) = prior.as_ref().filter(|c| c.verified_hash.is_some()) {
                    // A reconnect replays the entry completion at the durable
                    // EOF checkpoint.  Re-ack it without reopening or
                    // rewriting the already verified staged file.
                    match receive_control_or_cancel(&mut stream, &cancellation).await? {
                        ControlMessage::EntryComplete(done)
                            if done.batch_id == offer.batch_id
                                && done.entry_id == e.entry_id
                                && done.total_size == e.size
                                && prior.verified_hash.as_deref()
                                    == hex::decode(&done.blake3).ok().as_deref() =>
                        {
                            send_control_frame(
                                &mut stream,
                                ControlMessage::EntryVerified(EntryVerified {
                                    batch_id: offer.batch_id,
                                    entry_id: e.entry_id,
                                    relative_destination: e.components.clone(),
                                    blake3: done.blake3,
                                }),
                            )
                            .await?;
                            continue;
                        }
                        _ => return Err(receive_io("verified entry replay mismatch")),
                    }
                }
                let offset = prior.map(|c| c.durable_offset.max(0) as u64).unwrap_or(0);
                let mut file = if offset > 0 {
                    fileporter_transfer::ReceiverFile::resume(&area, &e.components, e.size, offset)
                } else {
                    fileporter_transfer::ReceiverFile::create(&area, &e.components, e.size)
                }
                .map_err(|_| receive_io("file staging failed"))?;
                loop {
                    match receive_frame_or_cancel(&mut stream, &cancellation).await? {
                        Frame::Chunk(c) => {
                            if c.batch_id != offer.batch_id || c.entry_id != e.entry_id {
                                return Err(receive_io("chunk identity mismatch"));
                            }
                            let settings = self.settings.clone();
                            let events = self.events.clone();
                            let t = target.clone();
                            let b = bid.clone();
                            let item = id.clone();
                            let p = peer.device_id.clone();
                            file.write_chunk(c.offset, &c.data, c.hash(), move |o| {
                                settings
                                    .save_checkpoint(&crate::persistence::Checkpoint {
                                        target_id: t.clone(),
                                        item_id: item,
                                        durable_offset: o as i64,
                                        verified_hash: None,
                                        updated_at: unix_timestamp(),
                                    })
                                    .map_err(|_| fileporter_transfer::TransferError::Durability)?;
                                settings
                                    .save_batch_target(&crate::persistence::BatchTarget {
                                        id: t,
                                        batch_id: b,
                                        peer_device_id: p,
                                        state: "receiving".into(),
                                        acknowledged_bytes: o as i64,
                                        error_code: None,
                                        retry_at: None,
                                        retry_count: 0,
                                        wait_for_available: false,
                                    })
                                    .map_err(|_| fileporter_transfer::TransferError::Durability)?;
                                events.emit(crate::state_events::StateEventKind::Progress);
                                Ok(())
                            })
                            .map_err(|error| match error {
                                fileporter_transfer::TransferError::Durability => {
                                    receive_io("checkpoint persistence failed")
                                }
                                fileporter_transfer::TransferError::DiskFull => {
                                    receive_io("disk full")
                                }
                                fileporter_transfer::TransferError::FsyncFailed => {
                                    receive_io("fsync failed")
                                }
                                _ => receive_io("invalid chunk"),
                            })?;
                            send_control_frame(
                                &mut stream,
                                ControlMessage::ChunkAck(fileporter_protocol::ChunkAck {
                                    batch_id: offer.batch_id,
                                    entry_id: e.entry_id,
                                    durable_offset: file.offset(),
                                }),
                            )
                            .await?;
                        }
                        Frame::Control(ControlMessage::EntryComplete(done)) => {
                            if done.batch_id != offer.batch_id
                                || done.entry_id != e.entry_id
                                || done.total_size != e.size
                            {
                                return Err(receive_io("entry completion mismatch"));
                            }
                            let hash: [u8; 32] = hex::decode(&done.blake3)
                                .ok()
                                .and_then(|v| v.try_into().ok())
                                .ok_or_else(|| receive_io("invalid entry hash"))?;
                            if file.complete(hash).is_err() {
                                // `complete` consumes and closes the handle. Mark
                                // this explicitly so the outer error path removes
                                // the owned root even if the peer closes next.
                                integrity_failed = true;
                                return Err(receive_io("entry hash mismatch"));
                            }
                            self.settings
                                .save_checkpoint(&crate::persistence::Checkpoint {
                                    target_id: target.clone(),
                                    item_id: id.clone(),
                                    durable_offset: e.size as i64,
                                    verified_hash: Some(hash.to_vec()),
                                    updated_at: unix_timestamp(),
                                })
                                .map_err(|_| receive_io("verification persistence failed"))?;
                            if let Ok(Some(mut item)) = self.settings.item(&id) {
                                item.state = "completed".into();
                                let _ = self.settings.save_item(&item);
                            }
                            send_control_frame(
                                &mut stream,
                                ControlMessage::EntryVerified(EntryVerified {
                                    batch_id: offer.batch_id,
                                    entry_id: e.entry_id,
                                    relative_destination: e.components.clone(),
                                    blake3: done.blake3,
                                }),
                            )
                            .await?;
                            break;
                        }
                        _ => return Err(receive_io("unexpected transfer frame")),
                    }
                }
            }
            match receive_control_or_cancel(&mut stream, &cancellation).await? {
                ControlMessage::BatchComplete(v) if v.batch_id == offer.batch_id => {}
                _ => return Err(receive_io("batch completion required")),
            }
            for e in page.entries.iter().filter(|e| e.parent_entry_id.is_none()) {
                let staged = area
                    .relative_path(&e.components)
                    .map_err(|_| receive_io("unsafe staged path"))?;
                let destination = if e.kind == EntryKind::Directory {
                    fileporter_transfer::mark_verified_tree(&staged)
                        .map_err(|_| receive_io("tree marker failed"))?;
                    fileporter_transfer::finalize_verified_directory_no_replace(
                        &staged,
                        root,
                        &e.components[0],
                    )
                } else {
                    fileporter_transfer::finalize_file_no_replace(&staged, root, &e.components[0])
                }
                .map_err(|_| receive_io("finalization failed"))?;
                self.finish_incoming(
                    &bid,
                    &target,
                    &e.entry_id.to_string(),
                    "completed",
                    Some(destination),
                );
            }
            send_control_frame(
                &mut stream,
                ControlMessage::BatchReceipt(BatchReceipt {
                    batch_id: offer.batch_id,
                    result: "completed".into(),
                }),
            )
            .await
        }
        .await;
        match work {
            Ok(()) => {
                let mut persisted = true;
                if let Ok(Some(mut b)) = self.settings.batch(&bid) {
                    b.state = "completed".into();
                    b.completed_at = Some(unix_timestamp());
                    persisted &= self.settings.save_batch(&b).is_ok();
                } else {
                    persisted = false;
                }
                if let Ok(Some(mut t)) = self.settings.batch_target(&target) {
                    t.state = "completed".into();
                    t.acknowledged_bytes = total as i64;
                    persisted &= self.settings.save_batch_target(&t).is_ok();
                } else {
                    persisted = false;
                }
                let _ = area.cleanup_owned();
                // Per-entry finalization already reports through
                // finish_incoming, but a batch with nothing to finalize (an
                // empty directory) reaches completion without any entry having
                // signalled. Announce the batch's own terminal transition so
                // that case still refreshes.
                if persisted {
                    self.events
                        .emit(crate::state_events::StateEventKind::Terminal);
                }
                Ok(())
            }
            Err(e) => {
                // A broken stream can reconnect safely.  Integrity/protocol
                // failures cannot: discard their staging tree so corrupt
                // prefixes never become resume candidates.
                let corrupt = e.to_string().contains("hash")
                    || e.to_string().contains("chunk")
                    || e.to_string().contains("manifest")
                    || e.to_string().contains("completion");
                if cancellation.is_cancelled() || corrupt || integrity_failed {
                    let _ = area.cleanup_owned();
                }
                self.finish_incoming(
                    &bid,
                    &target,
                    &page.entries[0].entry_id.to_string(),
                    if cancellation.is_cancelled() {
                        "cancelled"
                    } else {
                        "failed"
                    },
                    None,
                );
                Err(e)
            }
        }
    }

    #[allow(dead_code)] // Kept temporarily for the standalone single-file transport harness.
    async fn receive_one<S>(
        &self,
        mut stream: S,
        peer: fileporter_network::TrustedPeerPin,
        cancellation: CancellationToken,
    ) -> Result<(), ListenerError>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let settings = self
            .settings
            .load()
            .map_err(|_| receive_io("settings unavailable"))?;
        if !settings.receiving_enabled {
            return Err(receive_io("receiving disabled"));
        }
        let root = settings
            .receive_directory
            .ok_or_else(|| receive_io("receive root unavailable"))?;
        let offer = receive_control_or_cancel(&mut stream, &cancellation).await?;
        let ControlMessage::OfferStart(offer) = offer else {
            return Err(receive_io("transfer offer required"));
        };
        if offer.items.len() != 1
            || offer.total_entries != 1
            || offer.items[0].kind != EntryKind::File
            || offer.items[0].size != offer.total_bytes
            || offer.total_bytes > i64::MAX as u64
            || !safe_file_name(&offer.items[0].name)
        {
            return Err(receive_io("unsupported offer"));
        }
        let manifest = receive_control_or_cancel(&mut stream, &cancellation).await?;
        let ControlMessage::ManifestPage(manifest) = manifest else {
            return Err(receive_io("manifest required"));
        };
        if manifest.batch_id != offer.batch_id
            || !manifest.final_page
            || manifest.page != 0
            || manifest.entries.len() != 1
        {
            return Err(receive_io("invalid manifest"));
        }
        let entry = &manifest.entries[0];
        if entry.entry_id != offer.items[0].entry_id
            || entry.parent_entry_id.is_some()
            || entry.kind != EntryKind::File
            || entry.size != offer.total_bytes
            || entry.components != vec![offer.items[0].name.clone()]
        {
            return Err(receive_io("manifest does not match offer"));
        }
        // The sender never chooses a destination path.  Even this one-file
        // protocol slice maps its advertised name through the receiver's
        // platform policy before it is used below.
        let (destination_components, _warnings) =
            fileporter_transfer::sanitize_windows_components(&entry.components)
                .map_err(|_| receive_io("unsafe destination name"))?;
        let destination_name = destination_components
            .first()
            .cloned()
            .ok_or_else(|| receive_io("unsafe destination name"))?;
        if !receive_space_available(std::path::Path::new(&root), offer.total_bytes) {
            return Err(receive_io("insufficient disk space"));
        }
        let batch_id = offer.batch_id.to_string();
        let entry_id = entry.entry_id.to_string();
        let existing = self
            .settings
            .all_batches()
            .map_err(|_| receive_io("could not load incoming batch"))?
            .into_iter()
            .find(|record| record.batch.id == batch_id && record.batch.direction == "incoming");
        if existing
            .as_ref()
            .is_some_and(|record| record.batch.state == "completed")
        {
            // A receipt may have been lost after finalization.  Never recreate
            // the batch/staging area: that would permit a duplicate final file.
            return Err(receive_io("batch already finalized"));
        }
        let resumed_offset = existing
            .as_ref()
            .and_then(|_| {
                self.settings
                    .incoming_checkpoint(&batch_id, &entry_id)
                    .ok()
                    .flatten()
            })
            .map(|checkpoint| checkpoint.durable_offset.max(0) as u64)
            .unwrap_or(0);
        let area = if resumed_offset > 0 {
            fileporter_transfer::StagingArea::open(std::path::Path::new(&root), offer.batch_id)
        } else {
            fileporter_transfer::StagingArea::create(std::path::Path::new(&root), offer.batch_id)
        }
        .map_err(|_| receive_io("unsafe receive root or insufficient disk"))?;
        let target_id = existing
            .as_ref()
            .and_then(|record| record.targets.first())
            .map(|target| target.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let now = unix_timestamp();
        self.settings
            .save_batch(&crate::persistence::Batch {
                id: batch_id.clone(),
                direction: "incoming".into(),
                state: "receiving".into(),
                created_at: now,
                completed_at: None,
                total_bytes: offer.total_bytes as i64,
                total_entries: 1,
                warning_count: 0,
                wait_for_available: false,
            })
            .map_err(|_| receive_io("could not persist incoming batch"))?;
        self.settings
            .save_batch_target(&crate::persistence::BatchTarget {
                id: target_id.clone(),
                batch_id: batch_id.clone(),
                peer_device_id: peer.device_id.clone(),
                state: "receiving".into(),
                acknowledged_bytes: 0,
                error_code: None,
                retry_at: None,
                retry_count: 0,
                wait_for_available: false,
            })
            .map_err(|_| receive_io("could not persist incoming target"))?;
        self.settings
            .save_item(&crate::persistence::TransferItem {
                id: entry_id.clone(),
                batch_id: batch_id.clone(),
                parent_item_id: None,
                kind: "file".into(),
                display_name: offer.items[0].name.clone(),
                source_path_local: None,
                destination_path_local: None,
                size: offer.total_bytes as i64,
                mtime: None,
                state: "receiving".into(),
                warning_json: None,
            })
            .map_err(|_| receive_io("could not persist incoming item"))?;
        send_control_frame(
            &mut stream,
            ControlMessage::OfferAccept(OfferAccept {
                batch_id: offer.batch_id,
                destination_generation: 0,
                resolved_top_level_names: vec![destination_name.clone()],
                available_space_ok: true,
                checkpoints: (resumed_offset > 0)
                    .then_some(fileporter_protocol::Checkpoint {
                        entry_id: entry.entry_id,
                        durable_offset: resumed_offset,
                    })
                    .into_iter()
                    .collect(),
            }),
        )
        .await?;
        let mut file = Some(
            if resumed_offset > 0 {
                fileporter_transfer::ReceiverFile::resume(
                    &area,
                    &destination_components,
                    entry.size,
                    resumed_offset,
                )
            } else {
                fileporter_transfer::ReceiverFile::create(
                    &area,
                    &destination_components,
                    entry.size,
                )
            }
            .map_err(|_| receive_io("could not create staging file"))?,
        );
        let result = async {
            loop {
                match receive_frame_or_cancel(&mut stream, &cancellation).await? {
                    Frame::Chunk(chunk) => {
                        if chunk.batch_id != offer.batch_id || chunk.entry_id != entry.entry_id {
                            return Err(receive_io("chunk identity mismatch"));
                        }
                        let hash = chunk.hash();
                        let settings = self.settings.clone();
                        let target_id = target_id.clone();
                        let entry_id = entry_id.clone();
                        let batch_id = batch_id.clone();
                        let peer_id = peer.device_id.clone();
                        let receiver_file = file
                            .as_mut()
                            .ok_or_else(|| receive_io("entry already complete"))?;
                        receiver_file
                            .write_chunk(chunk.offset, &chunk.data, hash, move |offset| {
                                settings
                                    .save_checkpoint(&crate::persistence::Checkpoint {
                                        target_id: target_id.clone(),
                                        item_id: entry_id.clone(),
                                        durable_offset: offset as i64,
                                        verified_hash: None,
                                        updated_at: unix_timestamp(),
                                    })
                                    .map_err(|_| fileporter_transfer::TransferError::Durability)?;
                                settings
                                    .save_batch_target(&crate::persistence::BatchTarget {
                                        id: target_id,
                                        batch_id,
                                        peer_device_id: peer_id,
                                        state: "receiving".into(),
                                        acknowledged_bytes: offset as i64,
                                        error_code: None,
                                        retry_at: None,
                                        retry_count: 0,
                                        wait_for_available: false,
                                    })
                                    .map_err(|_| fileporter_transfer::TransferError::Durability)
                            })
                            .map_err(|error| match error {
                                fileporter_transfer::TransferError::Durability => {
                                    receive_io("checkpoint persistence failed")
                                }
                                fileporter_transfer::TransferError::DiskFull => {
                                    receive_io("disk full")
                                }
                                fileporter_transfer::TransferError::FsyncFailed => {
                                    receive_io("fsync failed")
                                }
                                _ => receive_io("invalid chunk"),
                            })?;
                        send_control_frame(
                            &mut stream,
                            ControlMessage::ChunkAck(fileporter_protocol::ChunkAck {
                                batch_id: offer.batch_id,
                                entry_id: entry.entry_id,
                                durable_offset: receiver_file.offset(),
                            }),
                        )
                        .await?;
                    }
                    Frame::Control(ControlMessage::EntryComplete(done)) => {
                        if done.batch_id != offer.batch_id
                            || done.entry_id != entry.entry_id
                            || done.total_size != entry.size
                        {
                            return Err(receive_io("entry completion mismatch"));
                        }
                        let expected: [u8; 32] = hex::decode(&done.blake3)
                            .ok()
                            .and_then(|v| v.try_into().ok())
                            .ok_or_else(|| receive_io("invalid entry hash"))?;
                        let staged = file
                            .take()
                            .ok_or_else(|| receive_io("entry already complete"))?
                            .complete(expected)
                            .map_err(|_| receive_io("entry hash mismatch"))?;
                        let destination = fileporter_transfer::finalize_file_no_replace(
                            &staged,
                            std::path::Path::new(&root),
                            &destination_name,
                        )
                        .map_err(|_| receive_io("could not finalize file"))?;
                        send_control_frame(
                            &mut stream,
                            ControlMessage::EntryVerified(EntryVerified {
                                batch_id: offer.batch_id,
                                entry_id: entry.entry_id,
                                relative_destination: vec![destination
                                    .file_name()
                                    .unwrap()
                                    .to_string_lossy()
                                    .to_string()],
                                blake3: done.blake3,
                            }),
                        )
                        .await?;
                        match receive_control_or_cancel(&mut stream, &cancellation).await? {
                            ControlMessage::BatchComplete(v) if v.batch_id == offer.batch_id => {}
                            _ => return Err(receive_io("batch completion required")),
                        }
                        send_control_frame(
                            &mut stream,
                            ControlMessage::BatchReceipt(BatchReceipt {
                                batch_id: offer.batch_id,
                                result: "completed".into(),
                            }),
                        )
                        .await?;
                        return Ok(destination);
                    }
                    _ => return Err(receive_io("unexpected transfer frame")),
                }
            }
        }
        .await;
        match result {
            Ok(destination) => {
                self.finish_incoming(
                    &batch_id,
                    &target_id,
                    &entry_id,
                    "completed",
                    Some(destination),
                );
                let _ = area.cleanup_owned();
                Ok(())
            }
            Err(error) => {
                drop(file);
                // Keep a durably acknowledged staging prefix for a reconnect.
                // Explicit cancellation is the only error path that cleans it.
                if cancellation.is_cancelled()
                    || !matches!(&error, ListenerError::Io(inner) if matches!(inner.kind(), io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted | io::ErrorKind::TimedOut))
                {
                    let _ = area.cleanup_owned();
                }
                self.finish_incoming(
                    &batch_id,
                    &target_id,
                    &entry_id,
                    if cancellation.is_cancelled() {
                        "cancelled"
                    } else {
                        "failed"
                    },
                    None,
                );
                Err(error)
            }
        }
    }
    fn finish_incoming(
        &self,
        batch: &str,
        target: &str,
        item: &str,
        state: &str,
        destination: Option<std::path::PathBuf>,
    ) {
        let now = unix_timestamp();
        let mut persisted = true;
        if let Ok(Some(mut value)) = self.settings.batch(batch) {
            value.state = state.into();
            value.completed_at = Some(now);
            persisted &= self.settings.save_batch(&value).is_ok();
        } else {
            persisted = false;
        }
        if let Ok(Some(mut value)) = self.settings.batch_target(target) {
            value.state = state.into();
            value.error_code = (state != "completed").then(|| "receive_failed".into());
            persisted &= self.settings.save_batch_target(&value).is_ok();
        } else {
            persisted = false;
        }
        if let Ok(Some(mut value)) = self.settings.item(item) {
            value.state = state.into();
            if let Some(destination) = destination {
                value.destination_path_local = Some(destination.to_string_lossy().to_string());
            }
            persisted &= self.settings.save_item(&value).is_ok();
        } else {
            persisted = false;
        }
        if persisted {
            self.events
                .emit(crate::state_events::StateEventKind::Terminal);
        }
    }
}

fn item_components(
    item: &crate::persistence::TransferItem,
    all: &[crate::persistence::TransferItem],
) -> Option<Vec<String>> {
    let mut components = vec![item.display_name.clone()];
    let mut parent = item.parent_item_id.as_deref();
    while let Some(parent_id) = parent {
        let parent_item = all.iter().find(|candidate| candidate.id == parent_id)?;
        components.push(parent_item.display_name.clone());
        parent = parent_item.parent_item_id.as_deref();
        if components.len() > fileporter_transfer::MAX_DEPTH {
            return None;
        }
    }
    components.reverse();
    fileporter_transfer::validate_receiver_components(&components).ok()?;
    Some(components)
}

fn path_hash_matches(path: &std::path::Path, expected: &[u8]) -> bool {
    if expected.len() != 32 || !path.is_file() {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let Ok(read) = file.read(&mut buffer) else {
            return false;
        };
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    hasher.finalize().as_bytes() == expected
}

fn trusted_pin_from_record(
    peer: &crate::persistence::TrustedPeer,
) -> Result<fileporter_network::TrustedPeerPin, ()> {
    Ok(fileporter_network::TrustedPeerPin {
        device_id: peer.device_id.clone(),
        public_key: peer.public_key.clone().try_into().map_err(|_| ())?,
        certificate_fingerprint: hex::decode(
            peer.certificate_fingerprint
                .strip_prefix("blake3:")
                .or_else(|| peer.certificate_fingerprint.strip_prefix("sha256:"))
                .unwrap_or(&peer.certificate_fingerprint),
        )
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or(())?,
    })
}
fn receive_io(message: &'static str) -> ListenerError {
    ListenerError::Io(io::Error::new(io::ErrorKind::InvalidData, message))
}
fn unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Leaves staging overhead so a receiver does not accept an offer it cannot
/// durably flush and finalize. Windows is the supported desktop target; other
/// targets still rely on create/sync failure before accepting data.
fn receive_space_available(root: &std::path::Path, bytes: u64) -> bool {
    let required = bytes.saturating_add(1024 * 1024);
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        unsafe extern "system" {
            fn GetDiskFreeSpaceExW(
                directory_name: *const u16,
                available: *mut u64,
                total: *mut u64,
                free: *mut u64,
            ) -> i32;
        }
        let mut path: Vec<u16> = root.as_os_str().encode_wide().collect();
        path.push(0);
        let mut available = 0u64;
        // SAFETY: NUL-terminated UTF-16 path and valid writable output.
        unsafe {
            GetDiskFreeSpaceExW(
                path.as_ptr(),
                &mut available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) != 0
                && available >= required
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (root, required);
        true
    }
}
async fn receive_frame_or_cancel<S: tokio::io::AsyncRead + Unpin>(
    stream: &mut S,
    cancellation: &CancellationToken,
) -> Result<Frame, ListenerError> {
    tokio::select! { _ = cancellation.cancelled() => Err(cancelled_io()), value = fileporter_network::receive_frame(stream) => value.map_err(network_io) }
}
async fn receive_control_or_cancel<S: tokio::io::AsyncRead + Unpin>(
    stream: &mut S,
    cancellation: &CancellationToken,
) -> Result<ControlMessage, ListenerError> {
    match receive_frame_or_cancel(stream, cancellation).await? {
        Frame::Control(v) => Ok(v),
        Frame::Chunk(_) => Err(receive_io("unexpected chunk")),
    }
}

fn normalized_source(source: IpAddr) -> IpAddr {
    match source {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(address)),
        other => other,
    }
}

impl Engine {
    pub fn new(pairing: Arc<crate::identity::PairingCoordinator>) -> Self {
        Self {
            lifecycle: AtomicU8::new(DORMANT),
            listener: Mutex::new(None),
            active_connections: Arc::new(AtomicUsize::new(0)),
            pairing: Arc::new(PairingService {
                coordinator: pairing,
                active: Arc::new(Mutex::new(HashMap::new())),
                source_counts: Arc::new(Mutex::new(HashMap::new())),
                queued: AtomicUsize::new(0),
                cancellation: CancellationToken::new(),
            }),
            receiver: None,
        }
    }

    /// Constructs the application listener with access to the durable trust,
    /// receive-root, transfer-history, and checkpoint records.
    pub fn with_receiver(
        pairing: Arc<crate::identity::PairingCoordinator>,
        settings: Arc<crate::persistence::SettingsRepository>,
    ) -> Self {
        Self::with_receiver_and_events(pairing, settings, crate::state_events::StateEvents::noop())
    }
    pub fn with_receiver_and_events(
        pairing: Arc<crate::identity::PairingCoordinator>,
        settings: Arc<crate::persistence::SettingsRepository>,
        events: crate::state_events::StateEvents,
    ) -> Self {
        let mut engine = Self::new(pairing.clone());
        engine.receiver = Some(Arc::new(ReceiverService {
            settings,
            pairing,
            events,
        }));
        engine
    }

    /// Must run before listening so persisted receive state and app-owned
    /// staging agree before any sender is offered a resume checkpoint.
    pub fn reconcile_receiver_startup(&self) -> Result<(), crate::error::AppError> {
        if let Some(receiver) = &self.receiver {
            receiver.reconcile_startup()?;
        }
        Ok(())
    }

    pub async fn start_pairing_at_endpoint(
        &self,
        endpoint: SocketAddr,
        device_name: String,
    ) -> Result<crate::identity::PendingPairingView, ListenerError> {
        if self.lifecycle() == EngineLifecycle::ShutDown {
            return Err(ListenerError::ShuttingDown);
        }
        self.pairing
            .start_outgoing(endpoint, device_name, None)
            .await
    }

    pub async fn start_automatic_pairing_at_endpoint(
        &self,
        endpoint: SocketAddr,
        device_name: String,
        expected_device_id: &str,
        expected_fingerprint: &str,
    ) -> Result<crate::identity::PendingPairingView, ListenerError> {
        if self.lifecycle() == EngineLifecycle::ShutDown {
            return Err(ListenerError::ShuttingDown);
        }
        self.pairing
            .start_outgoing(
                endpoint,
                device_name,
                Some((expected_device_id, expected_fingerprint)),
            )
            .await
    }

    pub fn confirm_pairing(
        &self,
        pairing_id: &str,
    ) -> Result<crate::identity::PairingConfirmationView, crate::error::AppError> {
        let confirmation = self.pairing.coordinator.confirm(pairing_id)?;
        self.pairing.send(
            pairing_id,
            ControlMessage::PairConfirmed(fileporter_protocol::PairConfirmed {
                session_id: self.pairing.session_id(pairing_id)?,
            }),
        );
        Ok(confirmation)
    }

    pub fn reject_pairing(&self, pairing_id: &str) -> Result<(), crate::error::AppError> {
        let session_id = self.pairing.session_id(pairing_id).ok();
        self.pairing.coordinator.reject(pairing_id)?;
        if let Some(session_id) = session_id {
            self.pairing.send(
                pairing_id,
                ControlMessage::PairRejected(fileporter_protocol::PairRejected {
                    session_id,
                    reason_code: "rejected".into(),
                }),
            );
        }
        Ok(())
    }

    pub fn lifecycle(&self) -> EngineLifecycle {
        match self.lifecycle.load(Ordering::Acquire) {
            SHUT_DOWN => EngineLifecycle::ShutDown,
            _ => EngineLifecycle::Dormant,
        }
    }

    pub fn listener_status(&self) -> ListenerStatus {
        let endpoint = self
            .listener
            .lock()
            .expect("listener mutex poisoned")
            .as_ref()
            .map(|session| session.address);
        let listening = self.lifecycle() != EngineLifecycle::ShutDown && endpoint.is_some();
        ListenerStatus {
            listening,
            receiving: listening && self.active_connections.load(Ordering::Acquire) > 0,
            bound_endpoint: listening.then_some(endpoint).flatten(),
        }
    }

    /// Starts one listener after validating that it is bound to a local-network
    /// address. Port zero is allowed for tests and OS-assigned listener ports.
    pub async fn start_listener(&self, address: SocketAddr) -> Result<SocketAddr, ListenerError> {
        if !address.ip().is_unspecified() && !is_loopback_or_private(address.ip()) {
            return Err(ListenerError::InvalidAddress);
        }
        if self.lifecycle() == EngineLifecycle::ShutDown {
            return Err(ListenerError::ShuttingDown);
        }
        if let Some(session) = self
            .listener
            .lock()
            .expect("listener mutex poisoned")
            .as_ref()
        {
            return Err(ListenerError::AlreadyListening(session.address));
        }
        let listener = TcpListener::bind(address).await?;
        let local_address = listener.local_addr()?;
        let mut listener_slot = self.listener.lock().expect("listener mutex poisoned");
        if let Some(session) = listener_slot.as_ref() {
            return Err(ListenerError::AlreadyListening(session.address));
        }
        let cancellation = CancellationToken::new();
        let connection_tasks = TaskTracker::new();
        let task = tokio::spawn(listener_loop(
            listener,
            cancellation.clone(),
            self.active_connections.clone(),
            connection_tasks.clone(),
            self.pairing.clone(),
            self.receiver.clone(),
        ));
        *listener_slot = Some(ListenerSession {
            address: local_address,
            cancellation,
            task,
            connection_tasks,
        });
        Ok(local_address)
    }

    /// Cancels accepting and waits for the listener task to drop its socket.
    pub async fn shutdown_listener(&self) {
        let session = self
            .listener
            .lock()
            .expect("listener mutex poisoned")
            .take();
        if let Some(session) = session {
            session.cancellation.cancel();
            let _ = session.task.await;
            session.connection_tasks.close();
            session.connection_tasks.wait().await;
        }
    }

    /// Synchronously begins graceful shutdown. Call `shutdown_listener` from an
    /// async shutdown owner when it needs to await socket release.
    pub fn begin_shutdown(&self) {
        self.lifecycle.store(SHUT_DOWN, Ordering::Release);
        self.pairing.shutdown();
        if let Some(session) = self
            .listener
            .lock()
            .expect("listener mutex poisoned")
            .as_ref()
        {
            session.cancellation.cancel();
        }
    }

    /// Sends one ordinary (including zero-byte) file using an authenticated,
    /// pinned TLS session. Completion is returned only after the receiver has
    /// durably hashed, finalized without replacement, and receipted the item.
    pub async fn send_one_loopback_file(
        &self,
        request: LoopbackFileTransfer,
        mut progress: impl FnMut(TransferProgress),
    ) -> Result<(), ListenerError> {
        if request.endpoint.port() == 0 || !is_loopback_or_private(request.endpoint.ip()) {
            return Err(ListenerError::InvalidAddress);
        }
        if !request.source.is_file() || !safe_file_name(&request.display_name) {
            return Err(ListenerError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid file transfer",
            )));
        }
        let total = std::fs::metadata(&request.source)?.len();
        let (mut stream, authenticated) = connect_authenticated(
            request.endpoint,
            &request.local_certificate,
            TrustMode::Trusted(request.trusted_peer),
        )
        .await
        .map_err(network_io)?;
        if authenticated.authorization != fileporter_network::SessionAuthorization::Trusted {
            return Err(ListenerError::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "peer is not trusted",
            )));
        }
        send_control_frame(
            &mut stream,
            ControlMessage::OfferStart(OfferStart {
                batch_id: request.batch_id,
                items: vec![TopLevelItem {
                    entry_id: request.entry_id,
                    name: request.display_name.clone(),
                    kind: EntryKind::File,
                    size: total,
                }],
                total_bytes: total,
                total_entries: 1,
                created_at: "explicit-loopback".into(),
            }),
        )
        .await?;
        send_control_frame(
            &mut stream,
            ControlMessage::ManifestPage(ManifestPage {
                batch_id: request.batch_id,
                page: 0,
                final_page: true,
                entries: vec![ManifestEntry {
                    entry_id: request.entry_id,
                    parent_entry_id: None,
                    kind: EntryKind::File,
                    components: vec![request.display_name.clone()],
                    size: total,
                    mtime: None,
                }],
            }),
        )
        .await?;
        match receive_control_frame(&mut stream).await? {
            ControlMessage::OfferAccept(OfferAccept {
                batch_id,
                available_space_ok: true,
                checkpoints,
                ..
            }) if batch_id == request.batch_id
                && checkpoints
                    .iter()
                    .find(|checkpoint| checkpoint.entry_id == request.entry_id)
                    .map(|checkpoint| checkpoint.durable_offset)
                    .unwrap_or(0)
                    == request.resume_offset => {}
            _ => {
                return Err(ListenerError::Io(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "offer was not accepted",
                )))
            }
        }
        let bytes = std::fs::read(&request.source)?;
        if request.resume_offset > bytes.len() as u64 {
            return Err(ListenerError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "resume offset exceeds source",
            )));
        }
        let mut offset = request.resume_offset;
        if bytes.is_empty() && request.cancellation.is_cancelled() {
            return Err(cancelled_io());
        }
        for data in
            bytes[request.resume_offset as usize..].chunks(fileporter_protocol::MAX_CHUNK_DATA)
        {
            if request.cancellation.is_cancelled() {
                return Err(cancelled_io());
            }
            fileporter_network::send_frame(
                &mut stream,
                &Frame::Chunk(Chunk {
                    batch_id: request.batch_id,
                    entry_id: request.entry_id,
                    offset,
                    data: data.to_vec(),
                }),
            )
            .await
            .map_err(network_io)?;
            offset += data.len() as u64;
            match receive_control_frame(&mut stream).await? {
                ControlMessage::ChunkAck(ack)
                    if ack.batch_id == request.batch_id
                        && ack.entry_id == request.entry_id
                        && ack.durable_offset == offset =>
                {
                    progress(TransferProgress {
                        acknowledged_bytes: offset,
                        total_bytes: total,
                    });
                    if request.cancellation.is_cancelled() {
                        return Err(cancelled_io());
                    }
                }
                _ => {
                    return Err(ListenerError::Io(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid chunk acknowledgement",
                    )))
                }
            }
        }
        send_control_frame(
            &mut stream,
            ControlMessage::EntryComplete(EntryComplete {
                batch_id: request.batch_id,
                entry_id: request.entry_id,
                total_size: total,
                blake3: blake3::hash(&bytes).to_hex().to_string(),
            }),
        )
        .await?;
        match receive_control_frame(&mut stream).await? {
            ControlMessage::EntryVerified(EntryVerified {
                batch_id,
                entry_id,
                blake3,
                ..
            }) if batch_id == request.batch_id
                && entry_id == request.entry_id
                && blake3 == blake3::hash(&bytes).to_hex().to_string() => {}
            _ => {
                return Err(ListenerError::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "receiver did not verify entry",
                )))
            }
        }
        send_control_frame(
            &mut stream,
            ControlMessage::BatchComplete(BatchComplete {
                batch_id: request.batch_id,
            }),
        )
        .await?;
        match receive_control_frame(&mut stream).await? {
            ControlMessage::BatchReceipt(BatchReceipt { batch_id, result })
                if batch_id == request.batch_id && result == "completed" =>
            {
                Ok(())
            }
            _ => Err(ListenerError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "receiver did not receipt batch",
            ))),
        }
    }

    /// Sends a complete, bounded manifest over one authenticated session.
    /// Each file is independently acknowledged and can therefore resume at a
    /// later entry without retransmitting earlier verified files.
    pub async fn send_loopback_batch(
        &self,
        request: LoopbackBatchTransfer,
        mut progress: impl FnMut(uuid::Uuid, TransferProgress) -> Result<(), ListenerError>,
    ) -> Result<(), ListenerError> {
        use std::collections::{HashMap, HashSet};
        if request.endpoint.port() == 0 || !is_loopback_or_private(request.endpoint.ip()) {
            return Err(ListenerError::InvalidAddress);
        }
        if request.entries.is_empty() || request.entries.len() > fileporter_transfer::MAX_ENTRIES {
            return Err(receive_io("invalid batch manifest"));
        }
        let mut ids = HashSet::new();
        let mut total = 0u64;
        let mut top = Vec::new();
        for entry in &request.entries {
            fileporter_transfer::validate_receiver_components(&entry.components)
                .map_err(|_| receive_io("unsafe manifest path"))?;
            if !ids.insert(entry.entry_id)
                || entry.components.len() == 1 && entry.parent_entry_id.is_some()
            {
                return Err(receive_io("invalid manifest identity"));
            }
            if let Some(parent) = entry.parent_entry_id {
                if !ids.contains(&parent) {
                    return Err(receive_io("manifest parent order"));
                }
            } else {
                top.push(entry);
            }
            let canonical = entry.source.canonicalize()?;
            let metadata = std::fs::metadata(&canonical)?;
            let mtime_matches = |expected: Option<&String>| {
                let expected = expected.and_then(|value| value.parse::<i64>().ok());
                expected
                    .map(|expected| {
                        metadata
                            .modified()
                            .ok()
                            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|value| value.as_secs() as i64)
                            == Some(expected)
                    })
                    .unwrap_or(true)
            };
            match entry.kind {
                EntryKind::File => {
                    if !metadata.is_file()
                        || metadata.len() != entry.size
                        || !mtime_matches(entry.mtime.as_ref())
                    {
                        return Err(receive_io("source changed"));
                    }
                    total = total
                        .checked_add(entry.size)
                        .ok_or_else(|| receive_io("batch too large"))?;
                }
                EntryKind::Directory => {
                    if !metadata.is_dir() || entry.size != 0 || !mtime_matches(entry.mtime.as_ref())
                    {
                        return Err(receive_io("invalid directory entry"));
                    }
                }
            }
        }
        if top.is_empty() {
            return Err(receive_io("manifest has no roots"));
        }
        let (mut stream, authenticated) = connect_authenticated(
            request.endpoint,
            &request.local_certificate,
            TrustMode::Trusted(request.trusted_peer),
        )
        .await
        .map_err(network_io)?;
        if authenticated.authorization != fileporter_network::SessionAuthorization::Trusted {
            return Err(receive_io("peer is not trusted"));
        }
        send_control_frame(
            &mut stream,
            ControlMessage::OfferStart(OfferStart {
                batch_id: request.batch_id,
                items: top
                    .iter()
                    .map(|entry| TopLevelItem {
                        entry_id: entry.entry_id,
                        name: entry.components[0].clone(),
                        kind: entry.kind.clone(),
                        size: entry.size,
                    })
                    .collect(),
                total_bytes: total,
                total_entries: request.entries.len() as u64,
                created_at: "durable-batch".into(),
            }),
        )
        .await?;
        send_control_frame(
            &mut stream,
            ControlMessage::ManifestPage(ManifestPage {
                batch_id: request.batch_id,
                page: 0,
                final_page: true,
                entries: request
                    .entries
                    .iter()
                    .map(|entry| ManifestEntry {
                        entry_id: entry.entry_id,
                        parent_entry_id: entry.parent_entry_id,
                        kind: entry.kind.clone(),
                        components: entry.components.clone(),
                        size: entry.size,
                        mtime: entry.mtime.clone(),
                    })
                    .collect(),
            }),
        )
        .await?;
        let checkpoints: HashMap<_, _> = match receive_control_frame(&mut stream).await? {
            ControlMessage::OfferAccept(OfferAccept {
                batch_id,
                available_space_ok: true,
                checkpoints,
                ..
            }) if batch_id == request.batch_id => checkpoints
                .into_iter()
                .map(|v| (v.entry_id, v.durable_offset))
                .collect(),
            _ => return Err(receive_io("offer was not accepted")),
        };
        for entry in request.entries.iter().filter(|e| e.kind == EntryKind::File) {
            // Keep memory bounded even for very large sources.  The complete
            // hash is accumulated while the file is streamed from byte zero.
            let canonical = entry.source.canonicalize()?;
            let metadata = std::fs::metadata(&canonical)?;
            let mtime_matches = entry
                .mtime
                .as_ref()
                .and_then(|value| value.parse::<i64>().ok())
                .map(|expected| {
                    metadata
                        .modified()
                        .ok()
                        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|value| value.as_secs() as i64)
                        == Some(expected)
                })
                .unwrap_or(true);
            if !metadata.is_file() || metadata.len() != entry.size || !mtime_matches {
                return Err(receive_io("source changed"));
            }
            let mut file = std::fs::File::open(&entry.source)?;
            let mut buffer = vec![0u8; fileporter_protocol::MAX_CHUNK_DATA];
            let mut offset = *checkpoints.get(&entry.entry_id).unwrap_or(&0);
            if offset != entry.resume_offset || offset > entry.size {
                return Err(receive_io("invalid resume checkpoint"));
            }
            let mut position = 0u64;
            let mut hasher = blake3::Hasher::new();
            loop {
                if request.cancellation.is_cancelled() {
                    return Err(cancelled_io());
                }
                #[cfg(test)]
                MAX_SOURCE_READ_BUFFER_FOR_TEST.fetch_max(buffer.len(), Ordering::AcqRel);
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
                if position + count as u64 <= offset {
                    position += count as u64;
                    continue;
                }
                let start = offset.saturating_sub(position) as usize;
                let data = &buffer[start..count];
                fileporter_network::send_frame(
                    &mut stream,
                    &Frame::Chunk(Chunk {
                        batch_id: request.batch_id,
                        entry_id: entry.entry_id,
                        offset,
                        data: data.to_vec(),
                    }),
                )
                .await
                .map_err(network_io)?;
                offset += data.len() as u64;
                match receive_control_frame(&mut stream).await? {
                    ControlMessage::ChunkAck(ack)
                        if ack.batch_id == request.batch_id
                            && ack.entry_id == entry.entry_id
                            && ack.durable_offset == offset =>
                    {
                        progress(
                            entry.entry_id,
                            TransferProgress {
                                acknowledged_bytes: offset,
                                total_bytes: entry.size,
                            },
                        )?;
                        if request.cancellation.is_cancelled() {
                            return Err(cancelled_io());
                        }
                    }
                    _ => return Err(receive_io("invalid chunk acknowledgement")),
                }
                position += count as u64;
            }
            if position != entry.size {
                return Err(receive_io("source changed while reading"));
            }
            if request.cancellation.is_cancelled() {
                return Err(cancelled_io());
            }
            let hash = hasher.finalize().to_hex().to_string();
            send_control_frame(
                &mut stream,
                ControlMessage::EntryComplete(EntryComplete {
                    batch_id: request.batch_id,
                    entry_id: entry.entry_id,
                    total_size: entry.size,
                    blake3: hash.clone(),
                }),
            )
            .await?;
            match receive_control_frame(&mut stream).await? {
                ControlMessage::EntryVerified(v)
                    if v.batch_id == request.batch_id
                        && v.entry_id == entry.entry_id
                        && v.blake3 == hash =>
                {
                    progress(
                        entry.entry_id,
                        TransferProgress {
                            acknowledged_bytes: entry.size,
                            total_bytes: entry.size,
                        },
                    )?
                }
                _ => return Err(receive_io("receiver did not verify entry")),
            }
        }
        send_control_frame(
            &mut stream,
            ControlMessage::BatchComplete(BatchComplete {
                batch_id: request.batch_id,
            }),
        )
        .await?;
        match receive_control_frame(&mut stream).await? {
            ControlMessage::BatchReceipt(v)
                if v.batch_id == request.batch_id && v.result == "completed" =>
            {
                Ok(())
            }
            _ => Err(receive_io("receiver did not receipt batch")),
        }
    }
}

fn safe_file_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != ".."
}
fn cancelled_io() -> ListenerError {
    ListenerError::Io(io::Error::new(
        io::ErrorKind::Interrupted,
        "transfer cancelled",
    ))
}
fn network_io(error: fileporter_network::NetworkError) -> ListenerError {
    ListenerError::Io(io::Error::new(
        io::ErrorKind::ConnectionAborted,
        error.to_string(),
    ))
}
async fn send_control_frame<S: tokio::io::AsyncWrite + Unpin>(
    stream: &mut S,
    control: ControlMessage,
) -> Result<(), ListenerError> {
    fileporter_network::send_frame(stream, &Frame::Control(control))
        .await
        .map_err(network_io)
}
async fn receive_control_frame<S: tokio::io::AsyncRead + Unpin>(
    stream: &mut S,
) -> Result<ControlMessage, ListenerError> {
    match fileporter_network::receive_frame(stream)
        .await
        .map_err(network_io)?
    {
        Frame::Control(control) => Ok(control),
        Frame::Chunk(_) => Err(ListenerError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected chunk",
        ))),
    }
}

/// Accepts only addresses useful for direct LAN/manual connections.
pub fn is_loopback_or_private(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => address.is_loopback() || address.is_private(),
        IpAddr::V6(address) => {
            // fc00::/7 is IPv6 unique-local space.  Spell this out to retain
            // the crate's Rust 1.77 MSRV (Ipv6Addr::is_unique_local is newer).
            address.is_loopback() || address.octets()[0] & 0xfe == 0xfc
        }
    }
}

/// Parses a user-provided direct endpoint and rejects public, multicast,
/// unspecified, and malformed values before any connection attempt.
pub fn validate_manual_endpoint(endpoint: &str) -> Result<SocketAddr, ListenerError> {
    let address: SocketAddr = endpoint
        .parse()
        .map_err(|_| ListenerError::InvalidAddress)?;
    if address.port() == 0 || !is_loopback_or_private(address.ip()) {
        return Err(ListenerError::InvalidAddress);
    }
    Ok(address)
}

/// Validates a local listening address. Unlike a remote manual endpoint, port
/// zero is useful here because it asks the OS to choose an available port.
pub fn validate_listen_address(address: &str) -> Result<SocketAddr, ListenerError> {
    let address: SocketAddr = address.parse().map_err(|_| ListenerError::InvalidAddress)?;
    if !address.ip().is_unspecified() && !is_loopback_or_private(address.ip()) {
        return Err(ListenerError::InvalidAddress);
    }
    Ok(address)
}

async fn listener_loop(
    listener: TcpListener,
    cancellation: CancellationToken,
    active_connections: Arc<AtomicUsize>,
    connection_tasks: TaskTracker,
    pairing: Arc<PairingService>,
    receiver: Option<Arc<ReceiverService>>,
) {
    loop {
        let accepted = tokio::select! {
            _ = cancellation.cancelled() => break,
            result = listener.accept() => result,
        };
        let Ok((stream, peer)) = accepted else {
            if cancellation.is_cancelled() {
                break;
            }
            continue;
        };
        if !is_loopback_or_private(peer.ip()) {
            drop(stream);
            continue;
        }
        active_connections.fetch_add(1, Ordering::AcqRel);
        let connection_cancellation = cancellation.clone();
        let active_connections = active_connections.clone();
        let pairing = pairing.clone();
        let receiver = receiver.clone();
        let receive_cancellation = connection_cancellation.clone();
        connection_tasks.spawn(async move {
            let device_name = "Fileporter device".to_owned();
            tokio::select! {
                _ = connection_cancellation.cancelled() => {},
                _ = async move {
                    if let Some(receiver) = receiver {
                        receiver.accept_or_pair(stream, pairing, device_name, receive_cancellation).await;
                    } else {
                        pairing.accept_incoming(stream, peer.ip(), device_name).await;
                    }
                } => {},
            };
            active_connections.fetch_sub(1, Ordering::AcqRel);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fileporter_identity::DeviceIdentity;
    use fileporter_network::{accept_authenticated, server_config_for_client, TrustedPeerPin};
    use fileporter_protocol::{ChunkAck, OfferAccept};
    use fileporter_transfer::{finalize_file_no_replace, ReceiverFile, StagingArea};
    use std::fs;
    use std::sync::Arc;
    use tokio_rustls::TlsAcceptor;

    fn persistent_receiver(
        directory: &std::path::Path,
        enabled: bool,
    ) -> (
        Engine,
        Arc<crate::persistence::SettingsRepository>,
        Arc<crate::identity::PairingCoordinator>,
    ) {
        let repository = Arc::new(
            crate::persistence::SettingsRepository::open(directory.join("receiver.sqlite3"))
                .unwrap(),
        );
        repository
            .save(&crate::persistence::Settings {
                device_name: "Receiver".into(),
                receive_directory: Some(directory.to_string_lossy().to_string()),
                onboarding_complete: true,
                receiving_enabled: enabled,
                ..crate::persistence::Settings::default()
            })
            .unwrap();
        let pairing =
            Arc::new(crate::identity::PairingCoordinator::open(repository.clone()).unwrap());
        (
            Engine::with_receiver(pairing.clone(), repository.clone()),
            repository,
            pairing,
        )
    }

    fn persistent_receiver_with_events(
        directory: &std::path::Path,
    ) -> (
        Engine,
        Arc<crate::persistence::SettingsRepository>,
        Arc<crate::identity::PairingCoordinator>,
        tokio::sync::mpsc::Receiver<crate::state_events::StateEventKind>,
        crate::state_events::StateEventWorker,
    ) {
        let repository = Arc::new(
            crate::persistence::SettingsRepository::open(directory.join("receiver.sqlite3"))
                .unwrap(),
        );
        repository
            .save(&crate::persistence::Settings {
                device_name: "Receiver".into(),
                receive_directory: Some(directory.to_string_lossy().to_string()),
                onboarding_complete: true,
                receiving_enabled: true,
                ..crate::persistence::Settings::default()
            })
            .unwrap();
        let pairing =
            Arc::new(crate::identity::PairingCoordinator::open(repository.clone()).unwrap());
        let (events, rx, worker) = crate::state_events::StateEventWorker::bounded(16);
        (
            Engine::with_receiver_and_events(pairing.clone(), repository.clone(), events),
            repository,
            pairing,
            rx,
            worker,
        )
    }

    fn persist_pin(repository: &crate::persistence::SettingsRepository, pin: &TrustedPeerPin) {
        repository
            .upsert_trusted_peer(&crate::persistence::TrustedPeer {
                device_id: pin.device_id.clone(),
                public_key: pin.public_key.to_vec(),
                certificate_fingerprint: format!(
                    "blake3:{}",
                    hex::encode(pin.certificate_fingerprint)
                ),
                remote_name: "Sender".into(),
                local_alias: None,
                paired_at: 1,
                last_seen_at: None,
                auto_send: false,
                revoked_at: None,
                endpoint: Some("127.0.0.1:1".into()),
            })
            .unwrap();
    }

    fn save_recovery_file(
        repository: &crate::persistence::SettingsRepository,
        batch_id: uuid::Uuid,
        entry_id: uuid::Uuid,
        target_id: &str,
        size: i64,
    ) {
        // Recovery targets retain a foreign key to their trusted sender.
        repository
            .upsert_trusted_peer(&crate::persistence::TrustedPeer {
                device_id: "sender".into(),
                public_key: vec![1],
                certificate_fingerprint: "blake3:recovery-sender".into(),
                remote_name: "Sender".into(),
                local_alias: None,
                paired_at: 1,
                last_seen_at: None,
                auto_send: false,
                revoked_at: None,
                endpoint: Some("127.0.0.1:1".into()),
            })
            .unwrap();
        repository
            .save_batch(&crate::persistence::Batch {
                id: batch_id.to_string(),
                direction: "incoming".into(),
                state: "receiving".into(),
                created_at: 1,
                completed_at: None,
                total_bytes: size,
                total_entries: 1,
                warning_count: 0,
                wait_for_available: false,
            })
            .unwrap();
        repository
            .save_batch_target(&crate::persistence::BatchTarget {
                id: target_id.into(),
                batch_id: batch_id.to_string(),
                peer_device_id: "sender".into(),
                state: "receiving".into(),
                acknowledged_bytes: 0,
                error_code: None,
                retry_at: None,
                retry_count: 0,
                wait_for_available: false,
            })
            .unwrap();
        repository
            .save_item(&crate::persistence::TransferItem {
                id: entry_id.to_string(),
                batch_id: batch_id.to_string(),
                parent_item_id: None,
                kind: "file".into(),
                display_name: "recover.bin".into(),
                source_path_local: None,
                destination_path_local: None,
                size,
                mtime: None,
                state: "receiving".into(),
                warning_json: None,
            })
            .unwrap();
    }

    #[test]
    fn restart_after_ack_before_db_update_never_replays_a_false_ack() {
        let directory = tempfile::tempdir().unwrap();
        let (receiver, repository, _) = persistent_receiver(directory.path(), true);
        let batch_id = uuid::Uuid::new_v4();
        let entry_id = uuid::Uuid::new_v4();
        save_recovery_file(&repository, batch_id, entry_id, "target", 7);
        let area = StagingArea::create(directory.path(), batch_id).unwrap();
        let mut file = ReceiverFile::create(&area, &["recover.bin".into()], 7).unwrap();
        file.write_chunk(0, b"abc", *blake3::hash(b"abc").as_bytes(), |_| Ok(()))
            .unwrap();
        drop(file);
        repository
            .save_checkpoint(&crate::persistence::Checkpoint {
                target_id: "target".into(),
                item_id: entry_id.to_string(),
                durable_offset: 7,
                verified_hash: None,
                updated_at: 1,
            })
            .unwrap();
        receiver.reconcile_receiver_startup().unwrap();
        assert_eq!(
            repository
                .checkpoint("target", &entry_id.to_string())
                .unwrap()
                .unwrap()
                .durable_offset,
            3
        );
        assert!(area.root().join("recover.bin").is_file());
    }

    #[test]
    fn startup_reconciliation_removes_only_old_unreferenced_owned_staging() {
        let directory = tempfile::tempdir().unwrap();
        let (receiver, _, _) = persistent_receiver(directory.path(), true);
        let orphan_id = uuid::Uuid::new_v4();
        let orphan = StagingArea::create(directory.path(), orphan_id).unwrap();
        fs::write(orphan.root().join("partial.bin"), b"partial").unwrap();
        let user_file = directory.path().join("keep-user-file.txt");
        fs::write(&user_file, b"keep").unwrap();
        receiver
            .receiver
            .as_ref()
            .unwrap()
            .reconcile_startup_at(unix_timestamp() + 30 * 24 * 60 * 60 + 1)
            .unwrap();
        assert!(!orphan.root().exists());
        assert_eq!(fs::read(user_file).unwrap(), b"keep");
    }

    #[test]
    fn startup_reconciliation_marks_missing_staging_terminal_failed() {
        let directory = tempfile::tempdir().unwrap();
        let (receiver, repository, _) = persistent_receiver(directory.path(), true);
        let batch_id = uuid::Uuid::new_v4();
        let entry_id = uuid::Uuid::new_v4();
        save_recovery_file(&repository, batch_id, entry_id, "target", 1);
        receiver.reconcile_receiver_startup().unwrap();
        assert_eq!(
            repository
                .batch(&batch_id.to_string())
                .unwrap()
                .unwrap()
                .state,
            "failed"
        );
        assert_eq!(
            repository
                .batch_target("target")
                .unwrap()
                .unwrap()
                .error_code
                .as_deref(),
            Some("recovery_staging_missing")
        );
    }

    #[tokio::test]
    async fn a_completed_receive_announces_itself_so_the_ui_can_stop_showing_it_in_flight() {
        // The failure path reports through finish_incoming. Success used to
        // persist "completed" and emit nothing, so the file landed on disk
        // while the UI went on showing the transfer as still running.
        let directory = tempfile::tempdir().unwrap();
        let (receiver, repository, receiver_pairing, mut events, worker) =
            persistent_receiver_with_events(directory.path());
        let sender_repo = Arc::new(
            crate::persistence::SettingsRepository::open(directory.path().join("sender.sqlite3"))
                .unwrap(),
        );
        let sender_pairing =
            Arc::new(crate::identity::PairingCoordinator::open(sender_repo).unwrap());
        let sender = Engine::new(sender_pairing.clone());
        let sender_cert = sender_pairing.local_certificate();
        let receiver_cert = receiver_pairing.local_certificate();
        persist_pin(
            &repository,
            &TrustedPeerPin::from_binding(sender_cert.binding()),
        );
        let endpoint = receiver
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let source = directory.path().join("arrival-source");
        fs::write(&source, b"landed").unwrap();

        sender
            .send_one_loopback_file(
                LoopbackFileTransfer {
                    endpoint,
                    local_certificate: sender_pairing.local_certificate(),
                    trusted_peer: TrustedPeerPin::from_binding(receiver_cert.binding()),
                    batch_id: uuid::Uuid::new_v4(),
                    entry_id: uuid::Uuid::new_v4(),
                    source,
                    display_name: "arrival.txt".into(),
                    resume_offset: 0,
                    cancellation: CancellationToken::new(),
                },
                |_| {},
            )
            .await
            .unwrap();

        // The batch is durably complete...
        let completed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let done = repository
                    .all_batches()
                    .unwrap()
                    .iter()
                    .any(|v| v.batch.direction == "incoming" && v.batch.state == "completed");
                if done {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(completed.is_ok(), "the receive never reached completed");
        assert_eq!(
            fs::read(directory.path().join("arrival.txt")).unwrap(),
            b"landed"
        );

        // ...and it said so, which is the only reason the UI ever refreshes.
        let mut saw_terminal = false;
        while let Ok(Some(kind)) =
            tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await
        {
            if kind == crate::state_events::StateEventKind::Terminal {
                saw_terminal = true;
                break;
            }
        }
        assert!(
            saw_terminal,
            "a completed receive emitted no terminal event; the UI would keep showing it in flight"
        );
        receiver.shutdown_listener().await;
        worker.shutdown().await;
    }

    #[tokio::test]
    async fn persistent_receiver_accepts_normal_zero_byte_and_collision_without_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let (receiver, repository, receiver_pairing) = persistent_receiver(directory.path(), true);
        let sender_repo = Arc::new(
            crate::persistence::SettingsRepository::open(directory.path().join("sender.sqlite3"))
                .unwrap(),
        );
        let sender_pairing =
            Arc::new(crate::identity::PairingCoordinator::open(sender_repo).unwrap());
        let sender = Engine::new(sender_pairing.clone());
        let sender_cert = sender_pairing.local_certificate();
        let receiver_cert = receiver_pairing.local_certificate();
        persist_pin(
            &repository,
            &TrustedPeerPin::from_binding(sender_cert.binding()),
        );
        let endpoint = receiver
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        fs::write(directory.path().join("same.txt"), b"new").unwrap();
        fs::write(directory.path().join("same (1).txt"), b"existing").unwrap();
        for (name, source) in [
            ("same.txt", directory.path().join("same.txt")),
            ("zero.txt", {
                let p = directory.path().join("zero-source");
                fs::write(&p, b"").unwrap();
                p
            }),
        ] {
            sender
                .send_one_loopback_file(
                    LoopbackFileTransfer {
                        endpoint,
                        local_certificate: sender_pairing.local_certificate(),
                        trusted_peer: TrustedPeerPin::from_binding(receiver_cert.binding()),
                        batch_id: uuid::Uuid::new_v4(),
                        entry_id: uuid::Uuid::new_v4(),
                        source,
                        display_name: name.into(),
                        resume_offset: 0,
                        cancellation: CancellationToken::new(),
                    },
                    |_| {},
                )
                .await
                .unwrap();
        }
        assert_eq!(
            fs::read(directory.path().join("same (2).txt")).unwrap(),
            b"new"
        );
        assert_eq!(
            fs::metadata(directory.path().join("zero.txt"))
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            fs::read(directory.path().join("same (1).txt")).unwrap(),
            b"existing"
        );
        assert_eq!(
            repository
                .all_batches()
                .unwrap()
                .iter()
                .filter(|v| v.batch.direction == "incoming" && v.batch.state == "completed")
                .count(),
            2
        );
        receiver.shutdown_listener().await;
    }

    #[tokio::test]
    async fn persistent_receiver_accepts_multi_entry_tree_and_empty_directory() {
        let directory = tempfile::tempdir().unwrap();
        let source_directory = tempfile::tempdir().unwrap();
        let (receiver, repository, receiver_pairing) = persistent_receiver(directory.path(), true);
        let sender_repo = Arc::new(
            crate::persistence::SettingsRepository::open(directory.path().join("sender.sqlite3"))
                .unwrap(),
        );
        let sender_pairing =
            Arc::new(crate::identity::PairingCoordinator::open(sender_repo).unwrap());
        let sender = Engine::new(sender_pairing.clone());
        persist_pin(
            &repository,
            &TrustedPeerPin::from_binding(sender_pairing.local_certificate().binding()),
        );
        let source_root = source_directory.path().join("source-tree");
        fs::create_dir_all(source_root.join("empty")).unwrap();
        fs::write(source_root.join("first.txt"), b"first").unwrap();
        fs::write(source_root.join("second.txt"), b"second").unwrap();
        let root_id = uuid::Uuid::new_v4();
        let first_id = uuid::Uuid::new_v4();
        let empty_id = uuid::Uuid::new_v4();
        let second_id = uuid::Uuid::new_v4();
        let endpoint = receiver
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        sender
            .send_loopback_batch(
                LoopbackBatchTransfer {
                    endpoint,
                    local_certificate: sender_pairing.local_certificate(),
                    trusted_peer: TrustedPeerPin::from_binding(
                        receiver_pairing.local_certificate().binding(),
                    ),
                    batch_id: uuid::Uuid::new_v4(),
                    entries: vec![
                        LoopbackBatchEntry {
                            entry_id: root_id,
                            parent_entry_id: None,
                            kind: EntryKind::Directory,
                            components: vec!["Payload".into()],
                            source: source_root.clone(),
                            size: 0,
                            mtime: None,
                            resume_offset: 0,
                        },
                        LoopbackBatchEntry {
                            entry_id: first_id,
                            parent_entry_id: Some(root_id),
                            kind: EntryKind::File,
                            components: vec!["Payload".into(), "first.txt".into()],
                            source: source_root.join("first.txt"),
                            size: 5,
                            mtime: None,
                            resume_offset: 0,
                        },
                        LoopbackBatchEntry {
                            entry_id: empty_id,
                            parent_entry_id: Some(root_id),
                            kind: EntryKind::Directory,
                            components: vec!["Payload".into(), "empty".into()],
                            source: source_root.join("empty"),
                            size: 0,
                            mtime: None,
                            resume_offset: 0,
                        },
                        LoopbackBatchEntry {
                            entry_id: second_id,
                            parent_entry_id: Some(root_id),
                            kind: EntryKind::File,
                            components: vec!["Payload".into(), "second.txt".into()],
                            source: source_root.join("second.txt"),
                            size: 6,
                            mtime: None,
                            resume_offset: 0,
                        },
                    ],
                    cancellation: CancellationToken::new(),
                },
                |_, _| Ok(()),
            )
            .await
            .unwrap();
        assert_eq!(
            fs::read(directory.path().join("Payload/first.txt")).unwrap(),
            b"first"
        );
        assert_eq!(
            fs::read(directory.path().join("Payload/second.txt")).unwrap(),
            b"second"
        );
        assert!(directory.path().join("Payload/empty").is_dir());
        receiver.shutdown_listener().await;
    }

    #[tokio::test]
    async fn persistent_receiver_rejects_traversal_manifest_before_any_write() {
        let directory = tempfile::tempdir().unwrap();
        let (receiver, repository, receiver_pairing) = persistent_receiver(directory.path(), true);
        let sender_repo = Arc::new(
            crate::persistence::SettingsRepository::open(directory.path().join("sender.sqlite3"))
                .unwrap(),
        );
        let sender_pairing =
            Arc::new(crate::identity::PairingCoordinator::open(sender_repo).unwrap());
        persist_pin(
            &repository,
            &TrustedPeerPin::from_binding(sender_pairing.local_certificate().binding()),
        );
        let endpoint = receiver
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let (mut stream, _) = connect_authenticated(
            endpoint,
            &sender_pairing.local_certificate(),
            TrustMode::Trusted(TrustedPeerPin::from_binding(
                receiver_pairing.local_certificate().binding(),
            )),
        )
        .await
        .unwrap();
        let batch_id = uuid::Uuid::new_v4();
        let entry_id = uuid::Uuid::new_v4();
        send_control_frame(
            &mut stream,
            ControlMessage::OfferStart(OfferStart {
                batch_id,
                items: vec![TopLevelItem {
                    entry_id,
                    name: "escape.txt".into(),
                    kind: EntryKind::File,
                    size: 0,
                }],
                total_bytes: 0,
                total_entries: 1,
                created_at: "test".into(),
            }),
        )
        .await
        .unwrap();
        send_control_frame(
            &mut stream,
            ControlMessage::ManifestPage(ManifestPage {
                batch_id,
                page: 0,
                final_page: true,
                entries: vec![ManifestEntry {
                    entry_id,
                    parent_entry_id: None,
                    kind: EntryKind::File,
                    components: vec!["..".into(), "escape.txt".into()],
                    size: 0,
                    mtime: None,
                }],
            }),
        )
        .await
        .unwrap();
        assert!(receive_control_frame(&mut stream).await.is_err());
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(!directory.path().join("escape.txt").exists());
        assert!(
            fileporter_transfer::enumerate_abandoned_staging(directory.path())
                .unwrap()
                .is_empty()
        );
        assert!(repository.all_batches().unwrap().is_empty());
        receiver.shutdown_listener().await;
    }

    #[tokio::test]
    async fn persistent_batch_reconnect_resumes_later_item_without_duplicate_earlier_output() {
        let directory = tempfile::tempdir().unwrap();
        let source_directory = tempfile::tempdir().unwrap();
        let (receiver, repository, receiver_pairing) = persistent_receiver(directory.path(), true);
        let sender_repo = Arc::new(
            crate::persistence::SettingsRepository::open(directory.path().join("sender.sqlite3"))
                .unwrap(),
        );
        let sender_pairing =
            Arc::new(crate::identity::PairingCoordinator::open(sender_repo).unwrap());
        let sender = Engine::new(sender_pairing.clone());
        persist_pin(
            &repository,
            &TrustedPeerPin::from_binding(sender_pairing.local_certificate().binding()),
        );
        let source_root = source_directory.path().join("source");
        fs::create_dir(&source_root).unwrap();
        let first = source_root.join("first.txt");
        let second = source_root.join("second.bin");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, vec![4u8; fileporter_protocol::MAX_CHUNK_DATA + 9]).unwrap();
        let root_id = uuid::Uuid::new_v4();
        let first_id = uuid::Uuid::new_v4();
        let second_id = uuid::Uuid::new_v4();
        let batch_id = uuid::Uuid::new_v4();
        let endpoint = receiver
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let server_pin =
            TrustedPeerPin::from_binding(receiver_pairing.local_certificate().binding());
        let cancellation = CancellationToken::new();
        let cancel_after_second_ack = cancellation.clone();
        let interrupted = sender
            .send_loopback_batch(
                LoopbackBatchTransfer {
                    endpoint,
                    local_certificate: sender_pairing.local_certificate(),
                    trusted_peer: server_pin.clone(),
                    batch_id,
                    entries: vec![
                        LoopbackBatchEntry {
                            entry_id: root_id,
                            parent_entry_id: None,
                            kind: EntryKind::Directory,
                            components: vec!["Payload".into()],
                            source: source_root.clone(),
                            size: 0,
                            mtime: None,
                            resume_offset: 0,
                        },
                        LoopbackBatchEntry {
                            entry_id: first_id,
                            parent_entry_id: Some(root_id),
                            kind: EntryKind::File,
                            components: vec!["Payload".into(), "first.txt".into()],
                            source: first.clone(),
                            size: 5,
                            mtime: None,
                            resume_offset: 0,
                        },
                        LoopbackBatchEntry {
                            entry_id: second_id,
                            parent_entry_id: Some(root_id),
                            kind: EntryKind::File,
                            components: vec!["Payload".into(), "second.bin".into()],
                            source: second.clone(),
                            size: (fileporter_protocol::MAX_CHUNK_DATA + 9) as u64,
                            mtime: None,
                            resume_offset: 0,
                        },
                    ],
                    cancellation,
                },
                |entry_id, _| {
                    if entry_id == second_id {
                        cancel_after_second_ack.cancel();
                    }
                    Ok(())
                },
            )
            .await;
        assert!(
            matches!(interrupted, Err(ListenerError::Io(error)) if error.kind() == io::ErrorKind::Interrupted)
        );
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        sender
            .send_loopback_batch(
                LoopbackBatchTransfer {
                    endpoint,
                    local_certificate: sender_pairing.local_certificate(),
                    trusted_peer: server_pin,
                    batch_id,
                    entries: vec![
                        LoopbackBatchEntry {
                            entry_id: root_id,
                            parent_entry_id: None,
                            kind: EntryKind::Directory,
                            components: vec!["Payload".into()],
                            source: source_root.clone(),
                            size: 0,
                            mtime: None,
                            resume_offset: 0,
                        },
                        LoopbackBatchEntry {
                            entry_id: first_id,
                            parent_entry_id: Some(root_id),
                            kind: EntryKind::File,
                            components: vec!["Payload".into(), "first.txt".into()],
                            source: first,
                            size: 5,
                            mtime: None,
                            resume_offset: 5,
                        },
                        LoopbackBatchEntry {
                            entry_id: second_id,
                            parent_entry_id: Some(root_id),
                            kind: EntryKind::File,
                            components: vec!["Payload".into(), "second.bin".into()],
                            source: second,
                            size: (fileporter_protocol::MAX_CHUNK_DATA + 9) as u64,
                            mtime: None,
                            resume_offset: fileporter_protocol::MAX_CHUNK_DATA as u64,
                        },
                    ],
                    cancellation: CancellationToken::new(),
                },
                |_, _| Ok(()),
            )
            .await
            .unwrap();
        assert_eq!(
            fs::read(directory.path().join("Payload/first.txt")).unwrap(),
            b"first"
        );
        assert_eq!(
            fs::read(directory.path().join("Payload/second.bin"))
                .unwrap()
                .len(),
            fileporter_protocol::MAX_CHUNK_DATA + 9
        );
        assert!(!directory.path().join("Payload (1)").exists());
        receiver.shutdown_listener().await;
    }

    #[tokio::test]
    async fn persistent_receiver_whole_tree_collision_never_overwrites_existing_tree() {
        let directory = tempfile::tempdir().unwrap();
        let source_directory = tempfile::tempdir().unwrap();
        let (receiver, repository, receiver_pairing) = persistent_receiver(directory.path(), true);
        let sender_repo = Arc::new(
            crate::persistence::SettingsRepository::open(directory.path().join("sender.sqlite3"))
                .unwrap(),
        );
        let sender_pairing =
            Arc::new(crate::identity::PairingCoordinator::open(sender_repo).unwrap());
        let sender = Engine::new(sender_pairing.clone());
        persist_pin(
            &repository,
            &TrustedPeerPin::from_binding(sender_pairing.local_certificate().binding()),
        );
        let source_root = source_directory.path().join("source-tree");
        fs::create_dir_all(source_root.join("nested")).unwrap();
        fs::write(source_root.join("nested/new.txt"), b"new").unwrap();
        fs::create_dir_all(directory.path().join("Payload/nested")).unwrap();
        fs::write(directory.path().join("Payload/nested/new.txt"), b"existing").unwrap();
        let root_id = uuid::Uuid::new_v4();
        let nested_id = uuid::Uuid::new_v4();
        let file_id = uuid::Uuid::new_v4();
        let endpoint = receiver
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        sender
            .send_loopback_batch(
                LoopbackBatchTransfer {
                    endpoint,
                    local_certificate: sender_pairing.local_certificate(),
                    trusted_peer: TrustedPeerPin::from_binding(
                        receiver_pairing.local_certificate().binding(),
                    ),
                    batch_id: uuid::Uuid::new_v4(),
                    entries: vec![
                        LoopbackBatchEntry {
                            entry_id: root_id,
                            parent_entry_id: None,
                            kind: EntryKind::Directory,
                            components: vec!["Payload".into()],
                            source: source_root.clone(),
                            size: 0,
                            mtime: None,
                            resume_offset: 0,
                        },
                        LoopbackBatchEntry {
                            entry_id: nested_id,
                            parent_entry_id: Some(root_id),
                            kind: EntryKind::Directory,
                            components: vec!["Payload".into(), "nested".into()],
                            source: source_root.join("nested"),
                            size: 0,
                            mtime: None,
                            resume_offset: 0,
                        },
                        LoopbackBatchEntry {
                            entry_id: file_id,
                            parent_entry_id: Some(nested_id),
                            kind: EntryKind::File,
                            components: vec!["Payload".into(), "nested".into(), "new.txt".into()],
                            source: source_root.join("nested/new.txt"),
                            size: 3,
                            mtime: None,
                            resume_offset: 0,
                        },
                    ],
                    cancellation: CancellationToken::new(),
                },
                |_, _| Ok(()),
            )
            .await
            .unwrap();
        assert_eq!(
            fs::read(directory.path().join("Payload/nested/new.txt")).unwrap(),
            b"existing"
        );
        assert_eq!(
            fs::read(directory.path().join("Payload (1)/nested/new.txt")).unwrap(),
            b"new"
        );
        receiver.shutdown_listener().await;
    }

    #[tokio::test]
    async fn persistent_receiver_rejects_untrusted_and_disabled_receiving() {
        let directory = tempfile::tempdir().unwrap();
        let (receiver, repository, receiver_pairing) = persistent_receiver(directory.path(), false);
        let sender_repo = Arc::new(
            crate::persistence::SettingsRepository::open(directory.path().join("sender.sqlite3"))
                .unwrap(),
        );
        let sender_pairing =
            Arc::new(crate::identity::PairingCoordinator::open(sender_repo).unwrap());
        let sender = Engine::new(sender_pairing.clone());
        let source = directory.path().join("blocked.txt");
        fs::write(&source, b"blocked").unwrap();
        let endpoint = receiver
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let result = sender
            .send_one_loopback_file(
                LoopbackFileTransfer {
                    endpoint,
                    local_certificate: sender_pairing.local_certificate(),
                    trusted_peer: TrustedPeerPin::from_binding(
                        receiver_pairing.local_certificate().binding(),
                    ),
                    batch_id: uuid::Uuid::new_v4(),
                    entry_id: uuid::Uuid::new_v4(),
                    source,
                    display_name: "blocked.txt".into(),
                    resume_offset: 0,
                    cancellation: CancellationToken::new(),
                },
                |_| {},
            )
            .await;
        assert!(result.is_err());
        persist_pin(
            &repository,
            &TrustedPeerPin::from_binding(sender_pairing.local_certificate().binding()),
        );
        let retry_source = directory.path().join("disabled-source");
        fs::write(&retry_source, b"disabled").unwrap();
        let disabled = sender
            .send_one_loopback_file(
                LoopbackFileTransfer {
                    endpoint,
                    local_certificate: sender_pairing.local_certificate(),
                    trusted_peer: TrustedPeerPin::from_binding(
                        receiver_pairing.local_certificate().binding(),
                    ),
                    batch_id: uuid::Uuid::new_v4(),
                    entry_id: uuid::Uuid::new_v4(),
                    source: retry_source,
                    display_name: "disabled.txt".into(),
                    resume_offset: 0,
                    cancellation: CancellationToken::new(),
                },
                |_| {},
            )
            .await;
        assert!(disabled.is_err());
        assert!(!directory.path().join("disabled.txt").exists());
        assert!(
            !directory.path().join("blocked.txt").exists()
                || fs::read(directory.path().join("blocked.txt")).unwrap() == b"blocked"
        );
        receiver.shutdown_listener().await;
    }

    #[tokio::test]
    async fn persistent_receiver_corrupt_hash_fails_without_final_file() {
        let directory = tempfile::tempdir().unwrap();
        let (receiver, repository, receiver_pairing) = persistent_receiver(directory.path(), true);
        let sender_repo = Arc::new(
            crate::persistence::SettingsRepository::open(directory.path().join("sender.sqlite3"))
                .unwrap(),
        );
        let sender_pairing =
            Arc::new(crate::identity::PairingCoordinator::open(sender_repo).unwrap());
        let local = sender_pairing.local_certificate();
        let remote = receiver_pairing.local_certificate();
        persist_pin(&repository, &TrustedPeerPin::from_binding(local.binding()));
        let endpoint = receiver
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let (mut stream, _) = connect_authenticated(
            endpoint,
            &local,
            TrustMode::Trusted(TrustedPeerPin::from_binding(remote.binding())),
        )
        .await
        .unwrap();
        let batch_id = uuid::Uuid::new_v4();
        let entry_id = uuid::Uuid::new_v4();
        send_control_frame(
            &mut stream,
            ControlMessage::OfferStart(OfferStart {
                batch_id,
                items: vec![TopLevelItem {
                    entry_id,
                    name: "bad.txt".into(),
                    kind: EntryKind::File,
                    size: 1,
                }],
                total_bytes: 1,
                total_entries: 1,
                created_at: "test".into(),
            }),
        )
        .await
        .unwrap();
        send_control_frame(
            &mut stream,
            ControlMessage::ManifestPage(ManifestPage {
                batch_id,
                page: 0,
                final_page: true,
                entries: vec![ManifestEntry {
                    entry_id,
                    parent_entry_id: None,
                    kind: EntryKind::File,
                    components: vec!["bad.txt".into()],
                    size: 1,
                    mtime: None,
                }],
            }),
        )
        .await
        .unwrap();
        assert!(matches!(
            receive_control_frame(&mut stream).await.unwrap(),
            ControlMessage::OfferAccept(_)
        ));
        send_control_frame(
            &mut stream,
            ControlMessage::EntryComplete(EntryComplete {
                batch_id,
                entry_id,
                total_size: 1,
                blake3: blake3::hash(b"wrong").to_hex().to_string(),
            }),
        )
        .await
        .unwrap();
        let _ = receive_control_frame(&mut stream).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(!directory.path().join("bad.txt").exists());
        assert!(repository
            .all_batches()
            .unwrap()
            .iter()
            .any(|v| v.batch.id == batch_id.to_string() && v.batch.state == "failed"));
        assert!(
            fileporter_transfer::enumerate_abandoned_staging(directory.path())
                .unwrap()
                .is_empty()
        );
        receiver.shutdown_listener().await;
    }

    fn test_engine() -> Engine {
        pairing_test_engine().0
    }
    fn pairing_test_engine() -> (Engine, Arc<crate::identity::PairingCoordinator>) {
        pairing_test_engine_with_automatic(false)
    }

    fn pairing_test_engine_with_automatic(
        automatic_device_trust: bool,
    ) -> (Engine, Arc<crate::identity::PairingCoordinator>) {
        let directory =
            std::env::temp_dir().join(format!("fileporter-engine-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let repository = std::sync::Arc::new(
            crate::persistence::SettingsRepository::open(directory.join("state.sqlite3")).unwrap(),
        );
        let mut settings = repository.load().unwrap();
        settings.automatic_device_trust = automatic_device_trust;
        repository.save(&settings).unwrap();
        let pairing = Arc::new(crate::identity::PairingCoordinator::open(repository).unwrap());
        (Engine::new(pairing.clone()), pairing)
    }

    #[test]
    fn address_policy_only_allows_loopback_and_private_ranges() {
        assert!(validate_manual_endpoint("127.0.0.1:4242").is_ok());
        assert!(validate_manual_endpoint("10.0.0.5:4242").is_ok());
        assert!(validate_manual_endpoint("192.168.1.10:4242").is_ok());
        assert!(validate_manual_endpoint("[::1]:4242").is_ok());
        assert!(validate_manual_endpoint("[fd00::1]:4242").is_ok());
        assert!(validate_manual_endpoint("8.8.8.8:4242").is_err());
        assert!(validate_manual_endpoint("0.0.0.0:4242").is_err());
        assert!(validate_manual_endpoint("127.0.0.1:0").is_err());
        assert!(validate_listen_address("127.0.0.1:0").is_ok());
        assert!(validate_listen_address("0.0.0.0:0").is_ok());
        assert!(validate_listen_address("[::]:0").is_ok());
    }

    #[tokio::test]
    async fn loopback_listener_tracks_lifecycle_and_releases_its_port() {
        let engine = test_engine();
        let address = engine
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        assert_eq!(
            engine.listener_status(),
            ListenerStatus {
                listening: true,
                receiving: false,
                bound_endpoint: Some(address),
            }
        );
        let client = tokio::net::TcpStream::connect(address).await.unwrap();
        for _ in 0..32 {
            if engine.listener_status().receiving {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(engine.listener_status().receiving);
        drop(client);
        engine.shutdown_listener().await;
        assert_eq!(
            engine.listener_status(),
            ListenerStatus {
                listening: false,
                receiving: false,
                bound_endpoint: None,
            }
        );
        assert!(TcpListener::bind(address).await.is_ok());
    }

    #[tokio::test]
    async fn two_peer_pairing_has_matching_sas_and_never_authorizes_transfer() {
        let left =
            LocalCertificate::generate(&DeviceIdentity::from_secret_bytes([31; 32])).unwrap();
        let right =
            LocalCertificate::generate(&DeviceIdentity::from_secret_bytes([32; 32])).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let (server, client) = tokio::join!(
            accept_pairing(&listener, &right, "Right"),
            initiate_pairing(endpoint, &left, "Left")
        );
        let server = server.unwrap();
        let client = client.unwrap();
        assert_eq!(server.sas, client.sas);
        assert_eq!(server.session_id, client.session_id);
        assert_eq!(server.peer, TrustedPeerPin::from_binding(left.binding()));
        assert_eq!(client.peer, TrustedPeerPin::from_binding(right.binding()));
    }

    #[tokio::test]
    async fn two_peers_commit_trust_only_after_both_confirm_over_listener() {
        let (left, left_pairing) = pairing_test_engine();
        let (right, right_pairing) = pairing_test_engine();
        let endpoint = right
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let left_pending = left
            .start_pairing_at_endpoint(endpoint, "Left".into())
            .await
            .unwrap();
        let right_pending = loop {
            let pending = right_pairing.snapshot().unwrap().pending_pairings;
            if let Some(pending) = pending.into_iter().next() {
                break pending;
            }
            tokio::task::yield_now().await;
        };
        assert!(left_pending.sas_code.as_deref().is_some_and(|code| {
            code.len() == 7
                && code.as_bytes()[3] == b' '
                && code
                    .chars()
                    .filter(|character| character.is_ascii_digit())
                    .count()
                    == 6
        }));
        assert_eq!(left_pending.sas_code, right_pending.sas_code);
        assert!(left_pairing.snapshot().unwrap().trusted_devices.is_empty());
        assert!(right_pairing.snapshot().unwrap().trusted_devices.is_empty());
        left.confirm_pairing(&left_pending.id).unwrap();
        right.confirm_pairing(&right_pending.id).unwrap();
        for _ in 0..64 {
            if !left_pairing.snapshot().unwrap().trusted_devices.is_empty()
                && !right_pairing.snapshot().unwrap().trusted_devices.is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(left_pairing.snapshot().unwrap().trusted_devices.len(), 1);
        assert_eq!(right_pairing.snapshot().unwrap().trusted_devices.len(), 1);
        left.shutdown_listener().await;
        right.shutdown_listener().await;
    }

    #[tokio::test]
    async fn two_auto_enabled_peers_trust_after_identity_proof_without_user_confirmation() {
        let (left, left_pairing) = pairing_test_engine_with_automatic(true);
        let (right, right_pairing) = pairing_test_engine_with_automatic(true);
        let endpoint = right
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        left.start_pairing_at_endpoint(endpoint, "Left".into())
            .await
            .unwrap();
        for _ in 0..100 {
            if !left_pairing.snapshot().unwrap().trusted_devices.is_empty()
                && !right_pairing.snapshot().unwrap().trusted_devices.is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let left_snapshot = left_pairing.snapshot().unwrap();
        let right_snapshot = right_pairing.snapshot().unwrap();
        assert!(left_snapshot.pending_pairings.is_empty());
        assert!(right_snapshot.pending_pairings.is_empty());
        assert_eq!(left_snapshot.trusted_devices.len(), 1);
        assert_eq!(right_snapshot.trusted_devices.len(), 1);
        assert!(left_snapshot.trusted_devices[0].auto_send);
        assert!(right_snapshot.trusted_devices[0].auto_send);
        left.shutdown_listener().await;
        right.shutdown_listener().await;
    }

    #[tokio::test]
    async fn automatic_pairing_rejects_an_endpoint_with_a_different_discovered_identity() {
        let (left, left_pairing) = pairing_test_engine_with_automatic(true);
        let (right, right_pairing) = pairing_test_engine_with_automatic(true);
        let endpoint = right
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let (_, right_fingerprint) = right_pairing.discovery_identity();
        assert!(left
            .start_automatic_pairing_at_endpoint(
                endpoint,
                "Left".into(),
                "different-device-id",
                &right_fingerprint,
            )
            .await
            .is_err());
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        assert!(left_pairing.snapshot().unwrap().trusted_devices.is_empty());
        assert!(right_pairing.snapshot().unwrap().trusted_devices.is_empty());
        left.shutdown_listener().await;
        right.shutdown_listener().await;
    }

    #[tokio::test]
    async fn confirmation_required_peer_prevents_automatic_trust_until_it_confirms() {
        let (left, left_pairing) = pairing_test_engine_with_automatic(true);
        let (right, right_pairing) = pairing_test_engine_with_automatic(false);
        let endpoint = right
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        left.start_pairing_at_endpoint(endpoint, "Left".into())
            .await
            .unwrap();
        let right_pending = loop {
            if let Some(pending) = right_pairing
                .snapshot()
                .unwrap()
                .pending_pairings
                .into_iter()
                .next()
            {
                if pending.remote_confirmed {
                    break pending;
                }
            }
            tokio::task::yield_now().await;
        };
        assert!(left_pairing.snapshot().unwrap().trusted_devices.is_empty());
        assert!(right_pairing.snapshot().unwrap().trusted_devices.is_empty());
        right.confirm_pairing(&right_pending.id).unwrap();
        for _ in 0..100 {
            if !left_pairing.snapshot().unwrap().trusted_devices.is_empty()
                && !right_pairing.snapshot().unwrap().trusted_devices.is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(left_pairing.snapshot().unwrap().trusted_devices.len(), 1);
        assert_eq!(right_pairing.snapshot().unwrap().trusted_devices.len(), 1);
        left.shutdown_listener().await;
        right.shutdown_listener().await;
    }

    #[tokio::test]
    async fn rejection_or_wrong_session_never_commits_trust() {
        let (left, left_pairing) = pairing_test_engine();
        let (right, right_pairing) = pairing_test_engine();
        let endpoint = right
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let left_pending = left
            .start_pairing_at_endpoint(endpoint, "Left".into())
            .await
            .unwrap();
        let right_pending = loop {
            if let Some(pending) = right_pairing
                .snapshot()
                .unwrap()
                .pending_pairings
                .into_iter()
                .next()
            {
                break pending;
            }
            tokio::task::yield_now().await;
        };
        left.confirm_pairing(&left_pending.id).unwrap();
        right.reject_pairing(&right_pending.id).unwrap();
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
        assert!(left_pairing.snapshot().unwrap().pending_pairings.is_empty());
        assert!(right_pairing
            .snapshot()
            .unwrap()
            .pending_pairings
            .is_empty());
        assert!(left_pairing.snapshot().unwrap().trusted_devices.is_empty());
        assert!(right_pairing.snapshot().unwrap().trusted_devices.is_empty());
        left.shutdown_listener().await;
        right.shutdown_listener().await;
    }

    #[tokio::test]
    async fn replayed_or_tampered_confirmation_session_is_rejected_without_trust() {
        let (left, left_pairing) = pairing_test_engine();
        let (right, right_pairing) = pairing_test_engine();
        let endpoint = right
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let left_pending = left
            .start_pairing_at_endpoint(endpoint, "Left".into())
            .await
            .unwrap();
        loop {
            if !right_pairing
                .snapshot()
                .unwrap()
                .pending_pairings
                .is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        left.pairing.send(
            &left_pending.id,
            ControlMessage::PairConfirmed(fileporter_protocol::PairConfirmed {
                session_id: uuid::Uuid::new_v4(),
            }),
        );
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
        assert!(left_pairing.snapshot().unwrap().trusted_devices.is_empty());
        assert!(right_pairing.snapshot().unwrap().trusted_devices.is_empty());
        left.shutdown_listener().await;
        right.shutdown_listener().await;
    }

    async fn receive_one_file(
        listener: TcpListener,
        acceptor: TlsAcceptor,
        local: LocalCertificate,
        pin: TrustedPeerPin,
        destination: std::path::PathBuf,
    ) -> Result<std::path::PathBuf, ListenerError> {
        let (mut stream, _) = accept_authenticated(&listener, acceptor, &local, pin)
            .await
            .map_err(network_io)?;
        let offer = match receive_control_frame(&mut stream).await? {
            ControlMessage::OfferStart(v) => v,
            _ => {
                return Err(ListenerError::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "expected offer",
                )))
            }
        };
        let manifest = match receive_control_frame(&mut stream).await? {
            ControlMessage::ManifestPage(mut v)
                if v.batch_id == offer.batch_id && v.entries.len() == 1 =>
            {
                v.entries.remove(0)
            }
            _ => {
                return Err(ListenerError::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "expected manifest",
                )))
            }
        };
        if manifest.kind != EntryKind::File
            || manifest.components.len() != 1
            || !safe_file_name(&manifest.components[0])
        {
            return Err(ListenerError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsafe manifest",
            )));
        }
        let area = StagingArea::create(&destination, offer.batch_id)
            .map_err(|_| ListenerError::Io(io::Error::other("staging failed")))?;
        send_control_frame(
            &mut stream,
            ControlMessage::OfferAccept(OfferAccept {
                batch_id: offer.batch_id,
                destination_generation: 0,
                resolved_top_level_names: vec![],
                available_space_ok: true,
                checkpoints: vec![],
            }),
        )
        .await?;
        let mut file = ReceiverFile::create(&area, &manifest.components, manifest.size)
            .map_err(|_| ListenerError::Io(io::Error::other("staging file failed")))?;
        loop {
            let frame = match fileporter_network::receive_frame(&mut stream).await {
                Ok(v) => v,
                Err(error) => {
                    drop(file);
                    let _ = area.cleanup_owned();
                    return Err(network_io(error));
                }
            };
            match frame {
                Frame::Chunk(chunk)
                    if chunk.batch_id == offer.batch_id && chunk.entry_id == manifest.entry_id =>
                {
                    let hash = chunk.hash();
                    file.write_chunk(chunk.offset, &chunk.data, hash, |_| Ok(()))
                        .map_err(|_| {
                            ListenerError::Io(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "bad chunk",
                            ))
                        })?;
                    send_control_frame(
                        &mut stream,
                        ControlMessage::ChunkAck(ChunkAck {
                            batch_id: offer.batch_id,
                            entry_id: manifest.entry_id,
                            durable_offset: file.offset(),
                        }),
                    )
                    .await?;
                }
                Frame::Control(ControlMessage::EntryComplete(done))
                    if done.batch_id == offer.batch_id && done.entry_id == manifest.entry_id =>
                {
                    let hash: [u8; 32] = hex::decode(&done.blake3)
                        .ok()
                        .and_then(|v| v.try_into().ok())
                        .ok_or_else(|| {
                            ListenerError::Io(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "bad hash",
                            ))
                        })?;
                    let staged = match file.complete(hash) {
                        Ok(staged) => staged,
                        Err(_) => {
                            // `complete` consumes and closes ReceiverFile.  Remove the
                            // owned staging root before returning so a peer that closes
                            // immediately after a bad hash cannot leave corrupt bytes
                            // behind for a later resume attempt.
                            let _ = area.cleanup_owned();
                            return Err(ListenerError::Io(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "hash mismatch",
                            )));
                        }
                    };
                    let final_path =
                        finalize_file_no_replace(&staged, &destination, &manifest.components[0])
                            .map_err(|_| {
                                ListenerError::Io(io::Error::other("finalization failed"))
                            })?;
                    send_control_frame(
                        &mut stream,
                        ControlMessage::EntryVerified(EntryVerified {
                            batch_id: offer.batch_id,
                            entry_id: manifest.entry_id,
                            relative_destination: vec![final_path
                                .file_name()
                                .unwrap()
                                .to_string_lossy()
                                .into_owned()],
                            blake3: done.blake3,
                        }),
                    )
                    .await?;
                    match receive_control_frame(&mut stream).await? {
                        ControlMessage::BatchComplete(v) if v.batch_id == offer.batch_id => {}
                        _ => {
                            return Err(ListenerError::Io(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "expected completion",
                            )))
                        }
                    };
                    send_control_frame(
                        &mut stream,
                        ControlMessage::BatchReceipt(BatchReceipt {
                            batch_id: offer.batch_id,
                            result: "completed".into(),
                        }),
                    )
                    .await?;
                    let _ = area.cleanup_owned();
                    return Ok(final_path);
                }
                _ => {
                    drop(file);
                    let _ = area.cleanup_owned();
                    return Err(ListenerError::Io(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unexpected frame",
                    )));
                }
            }
        }
    }

    #[tokio::test]
    async fn trusted_loopback_file_transfer_hashes_and_never_overwrites() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("report.txt");
        fs::write(&source, b"verified bytes").unwrap();
        let destination = directory.path().join("receive");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("report.txt"), b"existing").unwrap();
        let server_identity = DeviceIdentity::from_secret_bytes([41; 32]);
        let client_identity = DeviceIdentity::from_secret_bytes([42; 32]);
        let server = LocalCertificate::generate(&server_identity).unwrap();
        let client = LocalCertificate::generate(&client_identity).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let client_pin = TrustedPeerPin::from_binding(client.binding());
        let server_pin = TrustedPeerPin::from_binding(server.binding());
        let acceptor = TlsAcceptor::from(
            server_config_for_client(&server, &client_pin, client.certificate_der()).unwrap(),
        );
        let receiver = tokio::spawn(receive_one_file(
            listener,
            acceptor,
            server,
            client_pin,
            destination.clone(),
        ));
        let mut progress = Vec::new();
        test_engine()
            .send_one_loopback_file(
                LoopbackFileTransfer {
                    endpoint: address,
                    local_certificate: client,
                    trusted_peer: server_pin,
                    batch_id: uuid::Uuid::new_v4(),
                    entry_id: uuid::Uuid::new_v4(),
                    source,
                    display_name: "report.txt".into(),
                    resume_offset: 0,
                    cancellation: CancellationToken::new(),
                },
                |v| progress.push(v),
            )
            .await
            .unwrap();
        let final_path = receiver.await.unwrap().unwrap();
        assert_eq!(final_path.file_name().unwrap(), "report (1).txt");
        assert_eq!(fs::read(&final_path).unwrap(), b"verified bytes");
        assert_eq!(
            fs::read(destination.join("report.txt")).unwrap(),
            b"existing"
        );
        assert_eq!(progress.last().unwrap().acknowledged_bytes, 14);
    }

    #[tokio::test]
    async fn sender_emits_no_checkpoint_progress_before_matching_authenticated_ack() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("no-ack.bin");
        fs::write(&source, b"bytes").unwrap();
        let server_identity = DeviceIdentity::from_secret_bytes([81; 32]);
        let client_identity = DeviceIdentity::from_secret_bytes([82; 32]);
        let server = LocalCertificate::generate(&server_identity).unwrap();
        let client = LocalCertificate::generate(&client_identity).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let client_pin = TrustedPeerPin::from_binding(client.binding());
        let server_pin = TrustedPeerPin::from_binding(server.binding());
        let acceptor = TlsAcceptor::from(
            server_config_for_client(&server, &client_pin, client.certificate_der()).unwrap(),
        );
        let peer = tokio::spawn(async move {
            let (mut stream, _) = accept_authenticated(&listener, acceptor, &server, client_pin)
                .await
                .unwrap();
            let offer = match receive_control_frame(&mut stream).await.unwrap() {
                ControlMessage::OfferStart(value) => value,
                _ => panic!("offer"),
            };
            let manifest = match receive_control_frame(&mut stream).await.unwrap() {
                ControlMessage::ManifestPage(value) => value,
                _ => panic!("manifest"),
            };
            send_control_frame(
                &mut stream,
                ControlMessage::OfferAccept(OfferAccept {
                    batch_id: offer.batch_id,
                    destination_generation: 0,
                    resolved_top_level_names: vec![],
                    available_space_ok: true,
                    checkpoints: vec![],
                }),
            )
            .await
            .unwrap();
            match fileporter_network::receive_frame(&mut stream)
                .await
                .unwrap()
            {
                Frame::Chunk(_) => {}
                _ => panic!("chunk"),
            };
            // TLS is authenticated, but this protocol-level acknowledgement is
            // invalid and therefore must not reach the durable progress callback.
            send_control_frame(
                &mut stream,
                ControlMessage::ChunkAck(ChunkAck {
                    batch_id: offer.batch_id,
                    entry_id: manifest.entries[0].entry_id,
                    durable_offset: 0,
                }),
            )
            .await
            .unwrap();
        });
        let mut checkpoints = Vec::new();
        let result = test_engine()
            .send_one_loopback_file(
                LoopbackFileTransfer {
                    endpoint,
                    local_certificate: client,
                    trusted_peer: server_pin,
                    batch_id: uuid::Uuid::new_v4(),
                    entry_id: uuid::Uuid::new_v4(),
                    source,
                    display_name: "no-ack.bin".into(),
                    resume_offset: 0,
                    cancellation: CancellationToken::new(),
                },
                |progress| checkpoints.push(progress.acknowledged_bytes),
            )
            .await;
        assert!(result.is_err());
        assert!(
            checkpoints.is_empty(),
            "no durable checkpoint before a matching ChunkAck"
        );
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_after_staging_begins_leaves_no_finalized_item() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("cancel.bin");
        fs::write(&source, vec![7u8; fileporter_protocol::MAX_CHUNK_DATA + 1]).unwrap();
        let destination = directory.path().join("receive");
        fs::create_dir(&destination).unwrap();
        let server_identity = DeviceIdentity::from_secret_bytes([51; 32]);
        let client_identity = DeviceIdentity::from_secret_bytes([52; 32]);
        let server = LocalCertificate::generate(&server_identity).unwrap();
        let client = LocalCertificate::generate(&client_identity).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let client_pin = TrustedPeerPin::from_binding(client.binding());
        let server_pin = TrustedPeerPin::from_binding(server.binding());
        let acceptor = TlsAcceptor::from(
            server_config_for_client(&server, &client_pin, client.certificate_der()).unwrap(),
        );
        let receiver = tokio::spawn(receive_one_file(
            listener,
            acceptor,
            server,
            client_pin,
            destination.clone(),
        ));
        let cancellation = CancellationToken::new();
        let cancel_on_first_ack = cancellation.clone();
        let result = test_engine()
            .send_one_loopback_file(
                LoopbackFileTransfer {
                    endpoint: address,
                    local_certificate: client,
                    trusted_peer: server_pin,
                    batch_id: uuid::Uuid::new_v4(),
                    entry_id: uuid::Uuid::new_v4(),
                    source,
                    display_name: "cancel.bin".into(),
                    resume_offset: 0,
                    cancellation,
                },
                move |_| cancel_on_first_ack.cancel(),
            )
            .await;
        assert!(
            matches!(result, Err(ListenerError::Io(error)) if error.kind() == io::ErrorKind::Interrupted)
        );
        assert!(receiver.await.unwrap().is_err());
        assert!(!destination.join("cancel.bin").exists());
        let staging = destination.join(".fileporter-staging");
        assert!(fs::read_dir(staging).unwrap().next().is_none());
    }

    /// Fault-injected two-peer reconnect: the first authenticated ChunkAck is
    /// persisted by the receiver, the client connection is cancelled, and a
    /// fresh authenticated stream resumes from that exact durable boundary.
    #[tokio::test]
    async fn persistent_two_peer_resume_after_disconnect_finalizes_once() {
        let directory = tempfile::tempdir().unwrap();
        let source_directory = tempfile::tempdir().unwrap();
        let source = source_directory.path().join("resume.bin");
        fs::write(&source, vec![9u8; fileporter_protocol::MAX_CHUNK_DATA + 23]).unwrap();
        let (receiver, repository, pairing) = persistent_receiver(directory.path(), true);
        let sender_identity = DeviceIdentity::from_secret_bytes([71; 32]);
        let sender = LocalCertificate::generate(&sender_identity).unwrap();
        let sender_der = sender.persisted_der();
        let sender_pin = TrustedPeerPin::from_binding(sender.binding());
        persist_pin(&repository, &sender_pin);
        let endpoint = receiver
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let server_pin = TrustedPeerPin::from_binding(pairing.local_certificate().binding());
        let batch_id = uuid::Uuid::new_v4();
        let entry_id = uuid::Uuid::new_v4();
        let cancel = CancellationToken::new();
        let cancel_after_ack = cancel.clone();
        let mut acknowledged = 0;
        let interrupted = test_engine()
            .send_one_loopback_file(
                LoopbackFileTransfer {
                    endpoint,
                    local_certificate: sender,
                    trusted_peer: server_pin.clone(),
                    batch_id,
                    entry_id,
                    source: source.clone(),
                    display_name: "resume.bin".into(),
                    resume_offset: 0,
                    cancellation: cancel,
                },
                |progress| {
                    acknowledged = progress.acknowledged_bytes;
                    cancel_after_ack.cancel();
                },
            )
            .await;
        assert!(
            matches!(interrupted, Err(ListenerError::Io(error)) if error.kind() == io::ErrorKind::Interrupted)
        );
        assert_eq!(acknowledged, fileporter_protocol::MAX_CHUNK_DATA as u64);
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        let mut resumed = 0;
        let resumed_sender =
            LocalCertificate::from_persisted_der(&sender_identity, sender_der.0, sender_der.1)
                .unwrap();
        test_engine()
            .send_one_loopback_file(
                LoopbackFileTransfer {
                    endpoint,
                    local_certificate: resumed_sender,
                    trusted_peer: server_pin,
                    batch_id,
                    entry_id,
                    source,
                    display_name: "resume.bin".into(),
                    resume_offset: acknowledged,
                    cancellation: CancellationToken::new(),
                },
                |progress| resumed = progress.acknowledged_bytes,
            )
            .await
            .unwrap();
        assert_eq!(resumed, acknowledged + 23);
        assert_eq!(
            fs::read(directory.path().join("resume.bin")).unwrap().len(),
            (fileporter_protocol::MAX_CHUNK_DATA + 23)
        );
        assert!(!directory.path().join("resume (1).bin").exists());
        assert!(repository
            .all_batches()
            .unwrap()
            .iter()
            .any(|record| record.batch.id == batch_id.to_string()
                && record.batch.state == "completed"));
        receiver.shutdown_listener().await;
    }

    #[tokio::test]
    async fn trusted_loopback_zero_byte_file_is_verified_and_finalized() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("empty.txt");
        fs::write(&source, []).unwrap();
        let destination = directory.path().join("receive");
        fs::create_dir(&destination).unwrap();
        let server_identity = DeviceIdentity::from_secret_bytes([61; 32]);
        let client_identity = DeviceIdentity::from_secret_bytes([62; 32]);
        let server = LocalCertificate::generate(&server_identity).unwrap();
        let client = LocalCertificate::generate(&client_identity).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let client_pin = TrustedPeerPin::from_binding(client.binding());
        let server_pin = TrustedPeerPin::from_binding(server.binding());
        let acceptor = TlsAcceptor::from(
            server_config_for_client(&server, &client_pin, client.certificate_der()).unwrap(),
        );
        let receiver = tokio::spawn(receive_one_file(
            listener,
            acceptor,
            server,
            client_pin,
            destination.clone(),
        ));
        test_engine()
            .send_one_loopback_file(
                LoopbackFileTransfer {
                    endpoint: address,
                    local_certificate: client,
                    trusted_peer: server_pin,
                    batch_id: uuid::Uuid::new_v4(),
                    entry_id: uuid::Uuid::new_v4(),
                    source,
                    display_name: "empty.txt".into(),
                    resume_offset: 0,
                    cancellation: CancellationToken::new(),
                },
                |_| {},
            )
            .await
            .unwrap();
        assert_eq!(
            fs::metadata(receiver.await.unwrap().unwrap())
                .unwrap()
                .len(),
            0
        );
    }
}
