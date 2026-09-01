use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Four probes fit inside the presence window, so a single dropped packet or
/// momentary refusal never flips a healthy peer offline.
const PRESENCE_PROBE_EVERY: std::time::Duration = std::time::Duration::from_secs(10);
const PRESENCE_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

use crate::discovery::{
    resolve_advertised_endpoint, DiscoveryCoordinator, DiscoveryRecord, MdnsDiscoveryAdapter,
    CAPABILITIES, PROTOCOL_VERSION,
};
use crate::engine::{Engine, EngineLifecycle, ListenerStatus};
use crate::engine::{LoopbackBatchEntry, LoopbackBatchTransfer};
use crate::identity::{PairingCoordinator, PairingSnapshot};
use crate::listener_lifecycle::{ListenerAction, ListenerLifecycleCoordinator};
use crate::persistence::{
    Batch, BatchTarget, PersistedBatch, Settings, SettingsRepository, TransferItem,
};
use crate::state_events::{StateEventKind, StateEventWorker, StateEvents};
use fileporter_protocol::EntryKind;

#[derive(Clone)]
pub struct AppState {
    /// Kept independently of the webview; this scaffold does not run workers yet.
    pub engine: Arc<Engine>,
    pub settings: Arc<SettingsRepository>,
    pub pairing: Arc<PairingCoordinator>,
    discovery: Arc<DiscoveryCoordinator>,
    revision: Arc<AtomicU64>,
    shutting_down: Arc<AtomicBool>,
    shutdown_complete: Arc<AtomicBool>,
    suspended: Arc<AtomicBool>,
    scheduler: Arc<SenderScheduler>,
    events: StateEvents,
    event_worker: Arc<Mutex<Option<StateEventWorker>>>,
    automatic_pairing_inflight: Arc<Mutex<HashSet<String>>>,
    probing: Arc<AtomicBool>,
}

struct SenderScheduler {
    cancellation: CancellationToken,
    running: AtomicBool,
    active: Mutex<HashMap<String, CancellationToken>>,
    #[cfg(feature = "desktop")]
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    #[cfg(not(feature = "desktop"))]
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    wake: Notify,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub revision: u64,
    pub lifecycle: LifecycleSnapshot,
    pub settings: SettingsSnapshot,
    pub managers_started: bool,
    pub local_device_name: String,
    pub devices: Vec<DeviceViewModel>,
    pub nearby_devices: Vec<NearbyDeviceViewModel>,
    pub transfers: Vec<TransferBatchViewModel>,
    pub history: Vec<HistoryItemViewModel>,
    pub queued_batches: Vec<QueuedBatchDto>,
    pub pairing: PairingSnapshot,
    pub network: NetworkDiagnosticsView,
    pub about: AboutView,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleSnapshot {
    pub window_visible: bool,
    /// Persisted user preference; it does not assert a live listener exists.
    pub receiving_enabled: bool,
    /// Runtime listener state only. These do not claim device discovery.
    pub listening: bool,
    pub receiving: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound_endpoint: Option<String>,
    pub shutting_down: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    pub device_name: String,
    pub receive_directory: Option<String>,
    pub onboarding_complete: bool,
    pub launch_at_login: bool,
    pub notifications_enabled: bool,
    pub automatic_device_trust: bool,
    pub receiving_enabled: bool,
    pub preferred_listen_address: String,
    pub preferred_listen_port: u16,
    pub history_retention_days: i64,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkDiagnosticsView {
    pub listening: bool,
    pub bound_endpoint: Option<String>,
    pub preferred_listen_address: String,
    pub trusted_online_endpoints: Vec<String>,
    pub mdns_state: String,
    pub local_interface_summaries: Vec<String>,
    pub recent_error_codes: Vec<String>,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AboutView {
    pub app_version: String,
    pub protocol_version: u16,
    pub logs_available: bool,
    pub database_migration_version: i64,
    pub owned_staging_bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceViewModel {
    pub id: String,
    pub name: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<i64>,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NearbyDeviceViewModel {
    pub device_id: String,
    pub display_name: String,
    pub endpoint: String,
    pub certificate_fingerprint: String,
    pub protocol_version: u16,
    pub capabilities: Vec<String>,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TransferBatchViewModel {
    pub id: String,
    pub label: String,
    pub state: String,
    pub progress: u8,
    pub targets: Vec<TransferTargetViewModel>,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TransferTargetViewModel {
    pub id: String,
    pub device_name: String,
    pub state: String,
    pub progress: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_label: Option<String>,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItemViewModel {
    pub id: String,
    pub direction: String,
    pub peer_name: String,
    pub summary: String,
    pub time_label: String,
    pub state: String,
    pub items: Vec<HistoryTopLevelItemViewModel>,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryTopLevelItemViewModel {
    pub item_id: String,
    pub display_name: String,
    pub kind: String,
    pub size: i64,
    pub state: String,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination_label: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnqueuePathsRequest {
    pub paths: Vec<String>,
    pub target_device_ids: Vec<String>,
    pub queue_offline: bool,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QueuedBatchDto {
    pub id: String,
    pub item_count: usize,
    pub target_device_ids: Vec<String>,
    pub state: String,
    pub waiting_for_available: bool,
}

impl AppState {
    #[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Called only by the desktop command boundary.
    pub async fn start_pairing_discovered(
        &self,
        device_id: &str,
    ) -> Result<crate::identity::PendingPairingView, crate::error::AppError> {
        let candidate = self.discovery.candidate(device_id).ok_or_else(|| {
            crate::error::AppError::Validation {
                code: "nearby_device_not_found",
                message: "That nearby device is no longer available.",
                field: Some("deviceId"),
            }
        })?;
        let name = self.settings.load()?.device_name;
        self.engine
            .start_pairing_at_endpoint(candidate.record.endpoint, name)
            .await
            .map_err(|_| crate::error::AppError::ListenerUnavailable)
    }
    #[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Called only by the desktop command boundary.
    pub fn rename_trusted_device(
        &self,
        device_id: &str,
        alias: &str,
    ) -> Result<(), crate::error::AppError> {
        let alias = alias.trim();
        if alias.is_empty() || alias.chars().count() > 128 {
            return Err(crate::error::AppError::Validation {
                code: "invalid_device_alias",
                message: "Choose a device name up to 128 characters.",
                field: Some("alias"),
            });
        }
        let mut peer = self.settings.trusted_peer(device_id)?.ok_or_else(|| {
            crate::error::AppError::Validation {
                code: "device_not_found",
                message: "That trusted device is no longer available.",
                field: Some("deviceId"),
            }
        })?;
        peer.local_alias = Some(alias.into());
        self.settings.upsert_trusted_peer(&peer)
    }
    #[cfg(test)]
    pub fn new(settings: SettingsRepository) -> Self {
        Self::try_new(settings)
            .expect("local identity storage must be available when app state is created")
    }
    #[cfg(test)]
    pub fn try_new(settings: SettingsRepository) -> Result<Self, crate::error::AppError> {
        Self::try_new_with_events(settings, StateEvents::noop())
    }
    pub fn try_new_with_events(
        settings: SettingsRepository,
        events: StateEvents,
    ) -> Result<Self, crate::error::AppError> {
        let settings = Arc::new(settings);
        let pairing = Arc::new(PairingCoordinator::open(settings.clone())?);
        let engine = Arc::new(Engine::with_receiver_and_events(
            pairing.clone(),
            settings.clone(),
            events.clone(),
        ));
        engine.reconcile_receiver_startup()?;
        let discovery = Arc::new(DiscoveryCoordinator::new(Box::new(
            MdnsDiscoveryAdapter::new(),
        )));
        // Multicast is answered by every member of the group including this
        // one. Teach discovery its own identity before it ever browses.
        let (local_device_id, _) = pairing.discovery_identity();
        discovery.set_local_device_id(&local_device_id);
        Ok(Self {
            engine,
            settings,
            pairing,
            discovery,
            revision: Arc::new(AtomicU64::new(1)),
            shutting_down: Arc::new(AtomicBool::new(false)),
            shutdown_complete: Arc::new(AtomicBool::new(false)),
            suspended: Arc::new(AtomicBool::new(false)),
            scheduler: Arc::new(SenderScheduler {
                cancellation: CancellationToken::new(),
                running: AtomicBool::new(false),
                active: Mutex::new(HashMap::new()),
                task: Mutex::new(None),
                wake: Notify::new(),
            }),
            events,
            event_worker: Arc::new(Mutex::new(None)),
            automatic_pairing_inflight: Arc::new(Mutex::new(HashSet::new())),
            probing: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn snapshot(&self, window_visible: bool) -> Result<AppSnapshot, crate::error::AppError> {
        let settings = self.settings.load()?;
        let persisted = self.settings.all_batches()?;
        let pairing = self.pairing.snapshot()?;
        let listener = self.engine.listener_status();
        let queued = persisted
            .iter()
            .filter(|record| record.batch.state == "queued")
            .map(queued_dto_from_record)
            .collect();
        let diagnostic_errors = recent_error_codes(&persisted);
        let owned_staging_bytes = owned_staging_bytes(settings.receive_directory.as_deref());
        let mut snapshot = snapshot_from_settings(
            self.revision.load(Ordering::Relaxed),
            window_visible,
            listener,
            self.shutting_down.load(Ordering::Relaxed)
                || self.engine.lifecycle() == EngineLifecycle::ShutDown,
            settings,
            queued,
            persisted,
            pairing,
        );
        snapshot.devices = self
            .settings
            .active_trusted_peers()?
            .iter()
            .map(|peer| {
                let presence = self.discovery.snapshot(peer);
                DeviceViewModel {
                    id: peer.device_id.clone(),
                    name: peer
                        .local_alias
                        .clone()
                        .unwrap_or_else(|| peer.remote_name.clone()),
                    state: if presence.online { "online" } else { "offline" }.into(),
                    last_seen_at: presence.last_seen_at,
                }
            })
            .collect();
        snapshot.nearby_devices = self
            .discovery
            .nearby()
            .into_iter()
            .map(|candidate| NearbyDeviceViewModel {
                device_id: candidate.record.device_id,
                display_name: candidate.record.device_name,
                endpoint: candidate.record.endpoint.to_string(),
                certificate_fingerprint: candidate.record.certificate_fingerprint,
                protocol_version: candidate.record.protocol_version,
                capabilities: candidate.record.capabilities,
            })
            .collect();
        snapshot.network.trusted_online_endpoints = self
            .settings
            .active_trusted_peers()?
            .iter()
            .filter_map(|peer| {
                self.discovery
                    .snapshot(peer)
                    .online
                    .then(|| peer.endpoint.clone())
                    .flatten()
            })
            .collect();
        snapshot.network.mdns_state = self.discovery.mdns_state().as_str().into();
        snapshot.network.local_interface_summaries = local_interface_summaries(listener);
        snapshot.network.recent_error_codes =
            bounded_error_codes(self.discovery.recent_error_codes(), diagnostic_errors);
        snapshot.about.database_migration_version = self.settings.migration_version()?;
        snapshot.about.owned_staging_bytes = owned_staging_bytes;
        Ok(snapshot)
    }
    pub fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        self.engine.begin_shutdown();
        self.scheduler.cancellation.cancel();
        for token in self
            .scheduler
            .active
            .lock()
            .expect("sender scheduler mutex poisoned")
            .values()
        {
            token.cancel();
        }
    }
    pub fn shutdown_complete(&self) -> bool {
        self.shutdown_complete.load(Ordering::Acquire)
    }
    #[allow(dead_code)] // Reserved for a platform suspend callback; Tauri exposes no portable suspend RunEvent.
    pub async fn suspend(&self) {
        if self.suspended.swap(true, Ordering::AcqRel) {
            return;
        }
        for token in self
            .scheduler
            .active
            .lock()
            .expect("sender scheduler mutex poisoned")
            .values()
        {
            token.cancel();
        }
        self.engine.shutdown_listener().await;
        self.discovery.reconcile(None, &self.settings, unix_now());
    }
    #[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Desktop resume and network polling invoke this.
    pub async fn resume_or_network_changed(&self) -> Result<(), crate::error::AppError> {
        self.suspended.store(false, Ordering::Release);
        self.reconcile_listener().await?;
        self.start_sender_scheduler();
        for device_id in self.discovery.refresh(&self.settings, unix_now()) {
            self.wake_waiting_peer(&device_id)?;
        }
        self.start_automatic_pairings();
        Ok(())
    }
    /// Applies persisted listener preferences without claiming discovery or
    /// transfer scheduling. This may be called repeatedly after settings edits.
    pub async fn reconcile_listener(&self) -> Result<(), crate::error::AppError> {
        let settings = self.settings.load()?;
        let action = ListenerLifecycleCoordinator::action(
            settings.onboarding_complete,
            settings.receiving_enabled,
            self.shutting_down.load(Ordering::Acquire),
            &settings.listen_address,
            self.engine.listener_status(),
        )
        .map_err(|_| crate::error::AppError::ListenerUnavailable)?;
        match action {
            ListenerAction::Noop => {}
            ListenerAction::Start(address) => {
                self.engine
                    .start_listener(address)
                    .await
                    .map_err(|_| crate::error::AppError::ListenerUnavailable)?;
            }
            ListenerAction::Stop => self.engine.shutdown_listener().await,
            ListenerAction::Restart(address) => {
                self.engine.shutdown_listener().await;
                self.engine
                    .start_listener(address)
                    .await
                    .map_err(|_| crate::error::AppError::ListenerUnavailable)?;
            }
        }
        let listener = self.engine.listener_status();
        let local = if settings.onboarding_complete && settings.receiving_enabled {
            listener
                .bound_endpoint
                .and_then(resolve_advertised_endpoint)
                .map(|endpoint| {
                    let (device_id, certificate_fingerprint) = self.pairing.discovery_identity();
                    DiscoveryRecord {
                        device_id,
                        device_name: settings.device_name.clone(),
                        endpoint,
                        certificate_fingerprint,
                        protocol_version: PROTOCOL_VERSION,
                        capabilities: CAPABILITIES
                            .iter()
                            .map(|capability| (*capability).to_owned())
                            .collect(),
                    }
                })
        } else {
            None
        };
        let observed = self.discovery.reconcile(local, &self.settings, unix_now());
        for device_id in observed {
            self.wake_waiting_peer(&device_id)?;
        }
        self.start_automatic_pairings();
        Ok(())
    }
    /// Final shutdown always waits for the listener socket and its child work.
    pub async fn shutdown(&self) {
        self.begin_shutdown();
        let task = self
            .scheduler
            .task
            .lock()
            .expect("sender scheduler mutex poisoned")
            .take();
        if let Some(task) = task {
            let _ = task.await;
        }
        self.engine.shutdown_listener().await;
        let worker = {
            self.event_worker
                .lock()
                .expect("event worker mutex poisoned")
                .take()
        };
        if let Some(worker) = worker {
            worker.shutdown().await;
        }
        self.discovery.reconcile(None, &self.settings, unix_now());
        self.shutdown_complete.store(true, Ordering::Release);
    }
    #[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Desktop setup transfers ownership of its event worker here.
    pub fn attach_event_worker(&self, worker: StateEventWorker) {
        *self
            .event_worker
            .lock()
            .expect("event worker mutex poisoned") = Some(worker);
    }
    /// Starts one bounded durable sender loop.  It owns no discovery: each run
    /// snapshots the trusted record, endpoint, and TLS pin immediately before
    /// connecting.  A cancelled/shutdown loop is never restarted.
    pub fn start_sender_scheduler(&self) {
        if !self
            .settings
            .load()
            .map(|settings| settings.onboarding_complete)
            .unwrap_or(false)
        {
            return;
        }
        if self.scheduler.running.swap(true, Ordering::AcqRel)
            || self.scheduler.cancellation.is_cancelled()
        {
            return;
        }
        let state = self.clone();
        // Desktop setup is synchronous and is not entered into Tokio's reactor.
        // Use Tauri's owned runtime there; core tests already run inside Tokio.
        #[cfg(feature = "desktop")]
        let task = tauri::async_runtime::spawn(async move {
            state.run_sender_scheduler().await;
        });
        #[cfg(not(feature = "desktop"))]
        let task = tokio::spawn(async move {
            state.run_sender_scheduler().await;
        });
        *self
            .scheduler
            .task
            .lock()
            .expect("sender scheduler mutex poisoned") = Some(task);
    }

    async fn run_sender_scheduler(&self) {
        // Probe immediately on the first pass so a peer is not shown offline
        // for a whole interval after launch.
        let mut last_probe = std::time::Instant::now() - PRESENCE_PROBE_EVERY;
        loop {
            if self.scheduler.cancellation.is_cancelled() {
                break;
            }
            if self.suspended.load(Ordering::Acquire) {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                continue;
            }
            for device_id in self.discovery.refresh(&self.settings, unix_now()) {
                let _ = self.wake_waiting_peer(&device_id);
            }
            self.start_automatic_pairings();
            if last_probe.elapsed() >= PRESENCE_PROBE_EVERY {
                last_probe = std::time::Instant::now();
                self.spawn_presence_probe();
            }
            let candidates = self.settings.outgoing_batches().unwrap_or_default();
            for record in candidates {
                if self.scheduler.cancellation.is_cancelled() {
                    break;
                }
                if !matches!(record.batch.state.as_str(), "queued" | "sending")
                    || record.items.is_empty()
                {
                    continue;
                }
                if self
                    .scheduler
                    .active
                    .lock()
                    .expect("sender scheduler mutex poisoned")
                    .contains_key(&record.batch.id)
                {
                    continue;
                }
                if self.defer_unavailable_targets(&record).unwrap_or(false) {
                    continue;
                }
                if record
                    .targets
                    .iter()
                    .all(|target| target.retry_at.is_some_and(|at| at > unix_now()))
                {
                    continue;
                }
                let token = self.scheduler.cancellation.child_token();
                self.scheduler
                    .active
                    .lock()
                    .expect("sender scheduler mutex poisoned")
                    .insert(record.batch.id.clone(), token.clone());
                // Each worker persists its own target failure before it returns.
                // Do not overwrite an arbitrary sibling target here merely
                // because a fan-out aggregate returned Err.
                if self
                    .send_queued_file(&record.batch.id, token)
                    .await
                    .is_err()
                    && !record.batch.wait_for_available
                {
                    let _ = self.record_immediate_failure(&record.batch.id);
                }
                self.scheduler
                    .active
                    .lock()
                    .expect("sender scheduler mutex poisoned")
                    .remove(&record.batch.id);
            }
            tokio::select! { _ = self.scheduler.cancellation.cancelled() => break, _ = self.scheduler.wake.notified() => {}, _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {} }
        }
        self.scheduler.running.store(false, Ordering::Release);
    }

    /// Starts authenticated trust-on-first-discovery exchanges in the
    /// background. The lexically smaller identity goes first; the other side
    /// waits briefly and acts only as a fallback for asymmetric discovery.
    fn start_automatic_pairings(&self) {
        let Ok(settings) = self.settings.load() else {
            return;
        };
        if !settings.onboarding_complete
            || !settings.receiving_enabled
            || !settings.automatic_device_trust
        {
            return;
        }
        let (local_device_id, _) = self.pairing.discovery_identity();
        for candidate in self.discovery.nearby() {
            let device_id = candidate.record.device_id.clone();
            if self
                .settings
                .trusted_peer(&device_id)
                .map(|peer| peer.is_some())
                .unwrap_or(true)
                || self
                    .pairing
                    .snapshot()
                    .map(|snapshot| {
                        snapshot
                            .pending_pairings
                            .iter()
                            .any(|pairing| pairing.device_id == device_id)
                    })
                    .unwrap_or(true)
            {
                continue;
            }
            {
                let mut inflight = self
                    .automatic_pairing_inflight
                    .lock()
                    .expect("automatic pairing mutex poisoned");
                if !inflight.insert(device_id.clone()) {
                    continue;
                }
            }
            let state = self.clone();
            let preferred_initiator = local_device_id < device_id;
            tokio::spawn(async move {
                if !preferred_initiator {
                    tokio::time::sleep(std::time::Duration::from_millis(750)).await;
                }
                let should_connect = state
                    .settings
                    .load()
                    .map(|value| value.automatic_device_trust)
                    .unwrap_or(false)
                    && state
                        .settings
                        .trusted_peer(&device_id)
                        .map(|peer| peer.is_none())
                        .unwrap_or(false)
                    && state
                        .pairing
                        .snapshot()
                        .map(|snapshot| {
                            !snapshot
                                .pending_pairings
                                .iter()
                                .any(|pairing| pairing.device_id == device_id)
                        })
                        .unwrap_or(false);
                if should_connect {
                    let local_name = state
                        .settings
                        .load()
                        .map(|value| value.device_name)
                        .unwrap_or_default();
                    if state
                        .engine
                        .start_automatic_pairing_at_endpoint(
                            candidate.record.endpoint,
                            local_name,
                            &candidate.record.device_id,
                            &candidate.record.certificate_fingerprint,
                        )
                        .await
                        .is_ok()
                    {
                        for _ in 0..50 {
                            let trusted = state
                                .settings
                                .trusted_peer(&device_id)
                                .ok()
                                .flatten()
                                .is_some_and(|peer| {
                                    peer.revoked_at.is_none()
                                        && peer.certificate_fingerprint.eq_ignore_ascii_case(
                                            &candidate.record.certificate_fingerprint,
                                        )
                                });
                            if trusted {
                                if state.discovery.promote_trusted(
                                    &device_id,
                                    &state.settings,
                                    unix_now(),
                                ) {
                                    let _ = state.wake_waiting_peer(&device_id);
                                    state.bump_revision();
                                    state.events.emit(StateEventKind::Progress);
                                }
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    }
                }
                state
                    .automatic_pairing_inflight
                    .lock()
                    .expect("automatic pairing mutex poisoned")
                    .remove(&device_id);
            });
        }
    }
    /// Runs one reachability round off the scheduler loop so a slow or
    /// unreachable peer never delays queued transfers. Rounds never overlap.
    fn spawn_presence_probe(&self) {
        if self.probing.swap(true, Ordering::AcqRel) {
            return;
        }
        let state = self.clone();
        let round = async move {
            state.probe_presence().await;
            state.probing.store(false, Ordering::Release);
        };
        #[cfg(feature = "desktop")]
        tauri::async_runtime::spawn(round);
        #[cfg(not(feature = "desktop"))]
        tokio::spawn(round);
    }

    /// Presence follows whether the peer's pinned endpoint actually answers.
    /// The endpoint itself still comes from an identity-and-pin matched
    /// discovery record, and every transfer re-proves identity over TLS; this
    /// only decides whether the device is shown as reachable right now.
    async fn probe_presence(&self) {
        let Ok(peers) = self.settings.active_trusted_peers() else {
            return;
        };
        let mut changed = false;
        for peer in peers {
            let Some(endpoint) = peer
                .endpoint
                .as_deref()
                .and_then(|value| value.parse::<std::net::SocketAddr>().ok())
            else {
                continue;
            };
            let reachable = tokio::time::timeout(
                PRESENCE_PROBE_TIMEOUT,
                tokio::net::TcpStream::connect(endpoint),
            )
            .await
            .is_ok_and(|result| result.is_ok());
            let was_online = self.discovery.snapshot(&peer).online;
            let now = unix_now();
            self.discovery
                .record_reachability(&peer.device_id, reachable, now);
            if reachable {
                let mut refreshed = peer.clone();
                refreshed.last_seen_at = Some(now);
                let _ = self.settings.upsert_trusted_peer(&refreshed);
            }
            if reachable != was_online {
                changed = true;
                if reachable {
                    let _ = self.wake_waiting_peer(&peer.device_id);
                }
            }
        }
        if changed {
            self.bump_revision();
            self.events.emit(StateEventKind::Progress);
        }
    }

    pub fn cancel_batch(&self, batch_id: &str) -> Result<(), crate::error::AppError> {
        if let Some(token) = self
            .scheduler
            .active
            .lock()
            .expect("sender scheduler mutex poisoned")
            .get(batch_id)
        {
            token.cancel();
        }
        self.set_batch_terminal(batch_id, "cancelled", None)
    }
    /// Trusted discovery calls this after a pin-validated presence update. It
    /// makes wait-for-available targets eligible immediately instead of
    /// waiting for their persisted backoff deadline.
    pub fn wake_waiting_peer(&self, device_id: &str) -> Result<(), crate::error::AppError> {
        for mut record in self.settings.outgoing_batches()? {
            if !record.batch.wait_for_available || is_terminal(&record.batch.state) {
                continue;
            }
            let mut changed = false;
            for target in &mut record.targets {
                if target.peer_device_id == device_id && target.state == "waiting" {
                    target.state = "queued".into();
                    target.error_code = None;
                    target.retry_at = None;
                    self.settings.save_batch_target(target)?;
                    changed = true;
                }
            }
            if changed {
                self.bump_revision();
            }
        }
        self.scheduler.wake.notify_one();
        Ok(())
    }
    pub fn fail_targets_for_revoked_peer(
        &self,
        device_id: &str,
    ) -> Result<(), crate::error::AppError> {
        for mut record in self.settings.outgoing_batches()? {
            let mut changed = false;
            for target in &mut record.targets {
                if target.peer_device_id == device_id && !is_terminal(&target.state) {
                    target.state = "failed".into();
                    target.error_code = Some("recipient_revoked".into());
                    target.retry_at = None;
                    self.settings.save_batch_target(target)?;
                    changed = true;
                }
            }
            if changed {
                self.finish_batch_if_targets_terminal(&mut record.batch, &record.items)?;
            }
        }
        self.scheduler.wake.notify_one();
        self.bump_revision();
        Ok(())
    }
    fn defer_unavailable_targets(
        &self,
        record: &PersistedBatch,
    ) -> Result<bool, crate::error::AppError> {
        if !record.batch.wait_for_available {
            return Ok(false);
        }
        let mut deferred = false;
        let mut terminal_changed = false;
        let now = unix_now();
        for mut target in record.targets.clone() {
            if is_terminal(&target.state) || !target.wait_for_available {
                continue;
            }
            let peer = self.settings.trusted_peer(&target.peer_device_id)?;
            let Some(peer) = peer.filter(|peer| peer.revoked_at.is_none()) else {
                target.state = "failed".into();
                target.error_code = Some("recipient_revoked".into());
                target.retry_at = None;
                self.settings.save_batch_target(&target)?;
                terminal_changed = true;
                continue;
            };
            let online = self.discovery.snapshot(&peer).online && peer.endpoint.is_some();
            if !online {
                // A persisted deadline prevents a stale/offline presence from
                // incrementing retry state on every scheduler poll.
                if target.state == "waiting" && target.retry_at.is_some_and(|at| at > now) {
                    deferred = true;
                    continue;
                }
                target.state = "waiting".into();
                target.error_code = Some("waiting_for_available".into());
                target.retry_count = target.retry_count.saturating_add(1);
                let next = now.saturating_add(retry_delay_secs(target.retry_count));
                target.retry_at = Some(target.retry_at.unwrap_or_default().max(next));
                self.settings.save_batch_target(&target)?;
                deferred = true;
            }
        }
        if terminal_changed {
            let mut batch = record.batch.clone();
            self.finish_batch_if_targets_terminal(&mut batch, &record.items)?;
        }
        if deferred {
            self.bump_revision();
        }
        Ok(deferred)
    }
    fn record_immediate_failure(&self, batch_id: &str) -> Result<(), crate::error::AppError> {
        const MAX_IMMEDIATE_ATTEMPTS: i64 = 3;
        let Some(mut record) = self
            .settings
            .outgoing_batches()?
            .into_iter()
            .find(|record| record.batch.id == batch_id)
        else {
            return Ok(());
        };
        for target in &mut record.targets {
            if is_terminal(&target.state) {
                continue;
            }
            target.retry_count = target.retry_count.saturating_add(1);
            if target.retry_count >= MAX_IMMEDIATE_ATTEMPTS {
                target.state = "failed".into();
                target.error_code = Some("transfer_failed".into());
                target.retry_at = None;
            } else {
                target.state = "queued".into();
                target.error_code = Some("transfer_retrying".into());
                target.retry_at =
                    Some(unix_now().saturating_add(retry_delay_secs(target.retry_count)));
            }
            self.settings.save_batch_target(target)?;
        }
        self.finish_batch_if_targets_terminal(&mut record.batch, &record.items)?;
        self.bump_revision();
        Ok(())
    }
    pub fn retry_batch(&self, batch_id: &str) -> Result<(), crate::error::AppError> {
        let mut records = self.settings.outgoing_batches()?;
        let record = records
            .iter_mut()
            .find(|record| record.batch.id == batch_id)
            .ok_or(crate::error::AppError::Validation {
                code: "batch_not_found",
                message: "That transfer is no longer available.",
                field: Some("batchId"),
            })?;
        if !matches!(record.batch.state.as_str(), "failed" | "cancelled") {
            return Ok(());
        }
        record.batch.state = "queued".into();
        record.batch.completed_at = None;
        self.settings.save_batch(&record.batch)?;
        for target in &mut record.targets {
            target.state = "queued".into();
            target.error_code = None;
            target.retry_at = None;
            target.retry_count = 0;
            self.settings.save_batch_target(target)?;
        }
        for item in &mut record.items {
            item.state = "queued".into();
            self.settings.save_item(item)?;
        }
        self.bump_revision();
        Ok(())
    }
    fn set_batch_terminal(
        &self,
        batch_id: &str,
        state: &str,
        error: Option<&str>,
    ) -> Result<(), crate::error::AppError> {
        let mut records = self.settings.outgoing_batches()?;
        let record = records
            .iter_mut()
            .find(|record| record.batch.id == batch_id)
            .ok_or(crate::error::AppError::Validation {
                code: "batch_not_found",
                message: "That transfer is no longer available.",
                field: Some("batchId"),
            })?;
        if record.batch.state == "completed" {
            return Ok(());
        }
        record.batch.state = state.into();
        record.batch.completed_at = Some(unix_now());
        self.settings.save_batch(&record.batch)?;
        for target in &mut record.targets {
            target.state = state.into();
            target.error_code = error.map(str::to_owned);
            target.retry_at = None;
            self.settings.save_batch_target(target)?;
        }
        for item in &mut record.items {
            item.state = state.into();
            self.settings.save_item(item)?;
        }
        self.events.emit(StateEventKind::Terminal);
        self.bump_revision();
        Ok(())
    }
    pub fn bump_revision(&self) {
        self.revision.fetch_add(1, Ordering::SeqCst);
    }
    pub fn queue_batch(
        &self,
        request: EnqueuePathsRequest,
    ) -> Result<QueuedBatchDto, crate::error::AppError> {
        validate_enqueue_request(&request)?;
        let manifest = fileporter_transfer::build_source_manifest(
            request.paths.iter().map(Into::into).collect(),
            &CancellationToken::new(),
        )
        .map_err(manifest_error)?;
        if manifest.entries.is_empty() {
            return Err(crate::error::AppError::Validation {
                code: "unsupported_entry",
                message: "Choose at least one regular file or folder.",
                field: Some("paths"),
            });
        }
        let sequence = self.revision.fetch_add(1, Ordering::SeqCst);
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let unique_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = format!("batch-{unique_nanos}-{sequence}");
        let mut item_ids = std::collections::BTreeMap::new();
        for entry in &manifest.entries {
            item_ids.insert(entry.components.clone(), format!("{id}-{}", entry.id));
        }
        let items = manifest
            .entries
            .iter()
            .map(|entry| -> Result<TransferItem, crate::error::AppError> {
                let parent = if entry.components.len() > 1 {
                    item_ids
                        .get(&entry.components[..entry.components.len() - 1])
                        .cloned()
                } else {
                    None
                };
                Ok(TransferItem {
                    id: item_ids[&entry.components].clone(),
                    batch_id: id.clone(),
                    parent_item_id: parent,
                    kind: match entry.kind {
                        fileporter_transfer::EntryKind::File => "file",
                        fileporter_transfer::EntryKind::Directory => "directory",
                    }
                    .into(),
                    display_name: entry.components.last().cloned().unwrap_or_default(),
                    source_path_local: Some(entry.source_path_local.display().to_string()),
                    destination_path_local: None,
                    size: i64::try_from(entry.size).map_err(|_| {
                        crate::error::AppError::Validation {
                            code: "manifest_limit",
                            message: "Selected files exceed Fileporter's supported size.",
                            field: Some("paths"),
                        }
                    })?,
                    mtime: i64::try_from(entry.modified_unix_nanos / 1_000_000_000).ok(),
                    state: "queued".into(),
                    warning_json: None,
                })
            })
            .collect::<Result<Vec<_>, crate::error::AppError>>()?;
        let targets = request
            .target_device_ids
            .iter()
            .enumerate()
            .map(|(index, peer_device_id)| BatchTarget {
                id: format!("{id}-target-{index}"),
                batch_id: id.clone(),
                peer_device_id: peer_device_id.clone(),
                state: "queued".into(),
                acknowledged_bytes: 0,
                error_code: None,
                retry_at: None,
                retry_count: 0,
                wait_for_available: request.queue_offline,
            })
            .collect::<Vec<_>>();
        let batch = Batch {
            id: id.clone(),
            direction: "outgoing".into(),
            state: "queued".into(),
            created_at,
            completed_at: None,
            total_bytes: i64::try_from(manifest.total_logical_bytes).map_err(|_| {
                crate::error::AppError::Validation {
                    code: "manifest_limit",
                    message: "Selected files exceed Fileporter's supported size.",
                    field: Some("paths"),
                }
            })?,
            total_entries: items.len() as i64,
            warning_count: manifest.warnings.len() as i64,
            wait_for_available: request.queue_offline,
        };
        self.settings
            .enqueue_outgoing_batch(&batch, &targets, &items)?;
        Ok(QueuedBatchDto {
            id,
            item_count: items.len(),
            target_device_ids: request.target_device_ids,
            state: "queued".into(),
            waiting_for_available: false,
        })
    }

    async fn send_queued_batch_to_loopback_target(
        &self,
        batch_id: &str,
        target_id: &str,
        endpoint: std::net::SocketAddr,
        cancellation: CancellationToken,
    ) -> Result<(), crate::error::AppError> {
        let record = self
            .settings
            .outgoing_batches()?
            .into_iter()
            .find(|record| record.batch.id == batch_id)
            .ok_or(crate::error::AppError::Validation {
                code: "batch_not_found",
                message: "That transfer is no longer available.",
                field: Some("batchId"),
            })?;
        let mut target = record
            .targets
            .iter()
            .find(|target| target.id == target_id)
            .cloned()
            .ok_or(crate::error::AppError::Validation {
                code: "target_not_found",
                message: "That recipient is no longer part of this transfer.",
                field: Some("batchId"),
            })?;
        let peer = self
            .settings
            .trusted_peer(&target.peer_device_id)?
            .ok_or_else(|| crate::error::AppError::Validation {
                code: "unknown_recipient",
                message: "The selected device is not trusted.",
                field: Some("batchId"),
            })?;
        if peer.revoked_at.is_some() {
            return Err(crate::error::AppError::Validation {
                code: "unknown_recipient",
                message: "The selected device is not trusted.",
                field: Some("batchId"),
            });
        }
        let trusted_peer = trusted_peer_pin(&peer)?;
        let entries = durable_batch_entries(&record, &target, &self.settings)?;
        let mut batch = record.batch.clone();
        batch.state = "sending".into();
        target.state = "sending".into();
        self.settings.save_batch(&batch)?;
        self.settings.save_batch_target(&target)?;
        for item in &record.items {
            let mut sending = item.clone();
            sending.state = "sending".into();
            self.settings.save_item(&sending)?;
        }
        self.bump_revision();

        let mut offsets = std::collections::BTreeMap::new();
        for item in &record.items {
            offsets.insert(
                item.id.clone(),
                self.settings
                    .checkpoint(&target.id, &item.id)?
                    .map(|value| value.durable_offset.max(0))
                    .unwrap_or(0),
            );
        }
        let entry_items = record
            .items
            .iter()
            .map(|item| (stable_protocol_id(&item.id), item.id.clone()))
            .collect::<HashMap<_, _>>();
        let settings = self.settings.clone();
        let events = self.events.clone();
        let checkpoint_target = target.id.clone();
        let progress_target = target.clone();
        let mut acknowledged = offsets.values().copied().sum::<i64>();
        let result = self
            .engine
            .send_loopback_batch(
                LoopbackBatchTransfer {
                    endpoint,
                    local_certificate: self.pairing.local_certificate(),
                    trusted_peer,
                    batch_id: stable_protocol_id(&record.batch.id),
                    entries,
                    cancellation,
                },
                move |entry_id, progress| {
                    let item_id = entry_items.get(&entry_id).ok_or_else(|| {
                        crate::engine::ListenerError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "unknown progress entry",
                        ))
                    })?;
                    let prior = *offsets.get(item_id).unwrap_or(&0);
                    // save_checkpoint is one SQLite UPSERT.  Do this before reporting
                    // aggregate progress and deliberately fail the transfer if it fails.
                    settings
                        .save_checkpoint(&crate::persistence::Checkpoint {
                            target_id: checkpoint_target.clone(),
                            item_id: item_id.clone(),
                            durable_offset: progress.acknowledged_bytes as i64,
                            verified_hash: None,
                            updated_at: unix_now(),
                        })
                        .map_err(|error| {
                            crate::engine::ListenerError::Io(std::io::Error::other(
                                error.to_string(),
                            ))
                        })?;
                    offsets.insert(item_id.clone(), progress.acknowledged_bytes as i64);
                    acknowledged =
                        acknowledged.saturating_add(progress.acknowledged_bytes as i64 - prior);
                    let mut persisted = progress_target.clone();
                    persisted.acknowledged_bytes = acknowledged;
                    settings.save_batch_target(&persisted).map_err(|error| {
                        crate::engine::ListenerError::Io(std::io::Error::other(error.to_string()))
                    })?;
                    events.emit(StateEventKind::Progress);
                    Ok(())
                },
            )
            .await;
        target.state = if result.is_ok() { "completed" } else if result.as_ref().err().is_some_and(|error| matches!(error, crate::engine::ListenerError::Io(inner) if inner.kind() == std::io::ErrorKind::Interrupted)) { "cancelled" } else { "failed" }.into();
        target.acknowledged_bytes = acknowledged;
        target.error_code = result.as_ref().err().map(|_| "transfer_failed".into());
        self.settings.save_batch_target(&target)?;
        self.finish_batch_if_targets_terminal(&mut batch, &record.items)?;
        self.events.emit(StateEventKind::Terminal);
        self.bump_revision();
        result.map_err(|_| crate::error::AppError::Validation {
            code: "transfer_failed",
            message: "The transfer did not complete.",
            field: Some("batchId"),
        })
    }

    /// Sends using the endpoint stored with the already trusted peer.  This is
    /// the application path; the explicit-endpoint method remains for bounded
    /// loopback integration tests.
    pub async fn send_queued_file(
        &self,
        batch_id: &str,
        cancellation: CancellationToken,
    ) -> Result<(), crate::error::AppError> {
        let record = self
            .settings
            .outgoing_batches()?
            .into_iter()
            .find(|v| v.batch.id == batch_id)
            .ok_or(crate::error::AppError::Validation {
                code: "batch_not_found",
                message: "That transfer is no longer available.",
                field: Some("batchId"),
            })?;
        // A target owns its connection and durable checkpoint.  Keep the
        // bound intentionally small: v1 permits concurrent peers but never
        // lets one peer's socket failure cancel another's state machine.
        const MAX_CONCURRENT_TARGETS: usize = 2;
        let mut pending = std::collections::VecDeque::new();
        for target in record.targets {
            if matches!(target.state.as_str(), "completed" | "failed" | "cancelled") {
                continue;
            }
            let peer = self
                .settings
                .trusted_peer(&target.peer_device_id)?
                .ok_or_else(|| crate::error::AppError::Validation {
                    code: "unknown_recipient",
                    message: "The selected device is not trusted.",
                    field: Some("batchId"),
                })?;
            let endpoint = crate::engine::validate_manual_endpoint(
                peer.endpoint.as_deref().ok_or_else(invalid_endpoint)?,
            )
            .map_err(|_| invalid_endpoint())?;
            pending.push_back((target.id, endpoint));
        }
        let mut first_error = None;
        while !pending.is_empty() {
            let mut group = tokio::task::JoinSet::new();
            for _ in 0..MAX_CONCURRENT_TARGETS {
                let Some((target_id, endpoint)) = pending.pop_front() else {
                    break;
                };
                let state = self.clone();
                let batch = batch_id.to_owned();
                let target_cancellation = cancellation.child_token();
                group.spawn(async move {
                    state
                        .send_queued_batch_to_loopback_target(
                            &batch,
                            &target_id,
                            endpoint,
                            target_cancellation,
                        )
                        .await
                });
            }
            while let Some(joined) = group.join_next().await {
                match joined {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) if first_error.is_none() => first_error = Some(error),
                    Err(_) if first_error.is_none() => {
                        first_error = Some(crate::error::AppError::Validation {
                            code: "transfer_failed",
                            message: "A transfer worker stopped unexpectedly.",
                            field: Some("batchId"),
                        })
                    }
                    _ => {}
                }
            }
        }
        if let Some(error) = first_error {
            Err(error)
        } else {
            Ok(())
        }
    }

    fn finish_batch_if_targets_terminal(
        &self,
        batch: &mut Batch,
        items: &[TransferItem],
    ) -> Result<(), crate::error::AppError> {
        let targets = self.settings.batch_targets(&batch.id)?;
        if targets
            .iter()
            .any(|target| !matches!(target.state.as_str(), "completed" | "failed" | "cancelled"))
        {
            return Ok(());
        }
        batch.state = if targets.iter().all(|target| target.state == "completed") {
            "completed"
        } else if targets.iter().all(|target| target.state == "cancelled") {
            "cancelled"
        } else {
            "failed"
        }
        .into();
        batch.completed_at = Some(unix_now());
        self.settings.save_batch(batch)?;
        for item in items {
            let mut item = item.clone();
            item.state = batch.state.clone();
            self.settings.save_item(&item)?;
        }
        Ok(())
    }
}

/// Rebuild the receiver-relative manifest from the durable parent graph.  The
/// enqueue path is canonicalized before it is stored; the engine repeats that
/// check immediately before connecting, so a path replacement cannot escape
/// the original manifest.
fn durable_batch_entries(
    record: &PersistedBatch,
    target: &BatchTarget,
    settings: &SettingsRepository,
) -> Result<Vec<LoopbackBatchEntry>, crate::error::AppError> {
    let by_id = record
        .items
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect::<HashMap<_, _>>();
    fn components_for<'a>(
        item: &'a TransferItem,
        by_id: &HashMap<&'a str, &'a TransferItem>,
        visiting: &mut std::collections::HashSet<&'a str>,
    ) -> Result<Vec<String>, crate::error::AppError> {
        if !visiting.insert(item.id.as_str()) {
            return Err(crate::error::AppError::Validation {
                code: "invalid_manifest",
                message: "The queued transfer manifest is invalid.",
                field: Some("batchId"),
            });
        }
        let mut components = match item.parent_item_id.as_deref() {
            Some(parent) => components_for(
                by_id
                    .get(parent)
                    .copied()
                    .ok_or(crate::error::AppError::Validation {
                        code: "invalid_manifest",
                        message: "The queued transfer manifest is invalid.",
                        field: Some("batchId"),
                    })?,
                by_id,
                visiting,
            )?,
            None => Vec::new(),
        };
        visiting.remove(item.id.as_str());
        components.push(item.display_name.clone());
        fileporter_transfer::validate_receiver_components(&components).map_err(|_| {
            crate::error::AppError::Validation {
                code: "invalid_manifest",
                message: "The queued transfer manifest is invalid.",
                field: Some("batchId"),
            }
        })?;
        Ok(components)
    }
    let mut entries = Vec::with_capacity(record.items.len());
    for item in &record.items {
        let source = item
            .source_path_local
            .as_ref()
            .map(std::path::PathBuf::from)
            .ok_or(crate::error::AppError::Validation {
                code: "source_missing",
                message: "A queued source is no longer available.",
                field: Some("batchId"),
            })?;
        let mut visiting = std::collections::HashSet::new();
        let components = components_for(item, &by_id, &mut visiting)?;
        let resume_offset = settings
            .checkpoint(&target.id, &item.id)?
            .map(|value| value.durable_offset.max(0) as u64)
            .unwrap_or(0);
        entries.push((
            components.clone(),
            LoopbackBatchEntry {
                entry_id: stable_protocol_id(&item.id),
                parent_entry_id: item
                    .parent_item_id
                    .as_ref()
                    .map(|id| stable_protocol_id(id)),
                kind: if item.kind == "file" {
                    EntryKind::File
                } else {
                    EntryKind::Directory
                },
                components: components.clone(),
                source,
                size: u64::try_from(item.size).map_err(|_| crate::error::AppError::Validation {
                    code: "invalid_manifest",
                    message: "The queued transfer manifest is invalid.",
                    field: Some("batchId"),
                })?,
                mtime: item.mtime.map(|value| value.to_string()),
                resume_offset,
            },
        ));
    }
    // This is manifest order, not SQLite insertion/id order.  Lexical path
    // order is stable and places every directory before its descendants.
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries.into_iter().map(|(_, entry)| entry).collect())
}

fn invalid_endpoint() -> crate::error::AppError {
    crate::error::AppError::Validation {
        code: "invalid_endpoint",
        message: "The trusted device does not have a valid private-network endpoint.",
        field: Some("endpoint"),
    }
}

fn trusted_peer_pin(
    peer: &crate::persistence::TrustedPeer,
) -> Result<fileporter_network::TrustedPeerPin, crate::error::AppError> {
    let public_key = peer
        .public_key
        .clone()
        .try_into()
        .map_err(|_| invalid_peer_pin())?;
    let value = peer
        .certificate_fingerprint
        .strip_prefix("blake3:")
        .ok_or_else(invalid_peer_pin)?;
    let certificate_fingerprint = hex::decode(value)
        .ok()
        .and_then(|value| value.try_into().ok())
        .ok_or_else(invalid_peer_pin)?;
    Ok(fileporter_network::TrustedPeerPin {
        device_id: peer.device_id.clone(),
        public_key,
        certificate_fingerprint,
    })
}
fn invalid_peer_pin() -> crate::error::AppError {
    crate::error::AppError::Validation {
        code: "invalid_peer_pin",
        message: "The trusted device's certificate pin is invalid.",
        field: Some("batchId"),
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn retry_delay_secs(attempt: i64) -> i64 {
    // 1, 2, 4, ... seconds, capped at 256 seconds. Waiting policy has no
    // terminal retry budget; immediate sends retain their terminal failures.
    1_i64 << attempt.saturating_sub(1).clamp(0, 8)
}

fn stable_protocol_id(value: &str) -> uuid::Uuid {
    let hash = blake3::hash(value.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash.as_bytes()[..16]);
    uuid::Uuid::from_bytes(bytes)
}

#[allow(clippy::too_many_arguments)] // Snapshot composition intentionally keeps each independently-owned state source explicit.
fn snapshot_from_settings(
    revision: u64,
    window_visible: bool,
    listener: ListenerStatus,
    shutting_down: bool,
    settings: Settings,
    queued: Vec<QueuedBatchDto>,
    persisted: Vec<PersistedBatch>,
    pairing: PairingSnapshot,
) -> AppSnapshot {
    let local_device_name = if settings.device_name.is_empty() {
        "This device".to_owned()
    } else {
        settings.device_name.clone()
    };
    let preferred_listen_address = settings.listen_address.clone();
    let preferred_listen_port = preferred_listen_address
        .rsplit(':')
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    AppSnapshot {
        revision,
        managers_started: settings.onboarding_complete,
        lifecycle: LifecycleSnapshot {
            window_visible,
            receiving_enabled: settings.receiving_enabled,
            listening: listener.listening,
            receiving: listener.receiving,
            bound_endpoint: listener.bound_endpoint.map(|endpoint| endpoint.to_string()),
            shutting_down,
        },
        settings: SettingsSnapshot {
            device_name: settings.device_name,
            receive_directory: settings.receive_directory,
            onboarding_complete: settings.onboarding_complete,
            launch_at_login: settings.launch_at_login,
            notifications_enabled: settings.notifications_enabled,
            automatic_device_trust: settings.automatic_device_trust,
            receiving_enabled: settings.receiving_enabled,
            preferred_listen_port,
            preferred_listen_address: preferred_listen_address.clone(),
            history_retention_days: settings.history_retention_days,
        },
        local_device_name,
        devices: Vec::new(),
        nearby_devices: Vec::new(),
        transfers: persisted
            .iter()
            .filter(|record| !is_terminal(&record.batch.state))
            .map(transfer_from_record)
            .collect(),
        history: persisted
            .iter()
            .filter(|record| is_terminal(&record.batch.state))
            .map(history_from_record)
            .collect(),
        queued_batches: queued,
        pairing,
        network: NetworkDiagnosticsView {
            listening: listener.listening,
            bound_endpoint: listener.bound_endpoint.map(|value| value.to_string()),
            preferred_listen_address,
            trusted_online_endpoints: Vec::new(),
            mdns_state: "disabled".into(),
            local_interface_summaries: Vec::new(),
            recent_error_codes: Vec::new(),
        },
        about: AboutView {
            app_version: env!("CARGO_PKG_VERSION").into(),
            protocol_version: crate::discovery::PROTOCOL_VERSION,
            logs_available: true,
            database_migration_version: 0,
            owned_staging_bytes: 0,
        },
    }
}

fn local_interface_summaries(listener: ListenerStatus) -> Vec<String> {
    listener
        .bound_endpoint
        .and_then(resolve_advertised_endpoint)
        .map(|endpoint| vec![format!("bound:{}", endpoint.ip())])
        .unwrap_or_default()
}

fn recent_error_codes(persisted: &[PersistedBatch]) -> Vec<String> {
    let mut codes = Vec::new();
    for code in persisted
        .iter()
        .flat_map(|batch| &batch.targets)
        .filter_map(|target| target.error_code.as_deref())
    {
        if !codes.iter().any(|existing| existing == code) {
            codes.push(code.to_owned());
            if codes.len() == 16 {
                break;
            }
        }
    }
    codes
}

fn bounded_error_codes(mut first: Vec<String>, second: Vec<String>) -> Vec<String> {
    for code in second {
        if !first.iter().any(|existing| existing == &code) {
            first.push(code);
            if first.len() == 16 {
                break;
            }
        }
    }
    first
}

fn owned_staging_bytes(receive_directory: Option<&str>) -> u64 {
    let Some(directory) = receive_directory else {
        return 0;
    };
    staging_bytes(&std::path::Path::new(directory).join(".fileporter-staging"))
}

fn staging_bytes(directory: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let Ok(metadata) = entry.metadata() else {
                return 0;
            };
            if metadata.is_file() {
                metadata.len()
            } else if metadata.is_dir() {
                staging_bytes(&entry.path())
            } else {
                0
            }
        })
        .sum()
}

fn queued_dto_from_record(record: &PersistedBatch) -> QueuedBatchDto {
    QueuedBatchDto {
        id: record.batch.id.clone(),
        item_count: record.items.len(),
        target_device_ids: record
            .targets
            .iter()
            .map(|target| target.peer_device_id.clone())
            .collect(),
        state: record.batch.state.clone(),
        waiting_for_available: record.batch.wait_for_available
            && record
                .targets
                .iter()
                .any(|target| target.state == "waiting"),
    }
}
fn is_terminal(state: &str) -> bool {
    matches!(state, "completed" | "failed" | "cancelled")
}
fn transfer_from_record(record: &PersistedBatch) -> TransferBatchViewModel {
    TransferBatchViewModel {
        id: record.batch.id.clone(),
        label: format!(
            "{} item{}",
            record.items.len(),
            if record.items.len() == 1 { "" } else { "s" }
        ),
        state: record.batch.state.clone(),
        progress: progress(
            record.batch.total_bytes,
            record
                .targets
                .first()
                .map(|v| v.acknowledged_bytes)
                .unwrap_or(0),
        ),
        targets: record
            .targets
            .iter()
            .map(|target| TransferTargetViewModel {
                id: target.id.clone(),
                device_name: target.peer_device_id.clone(),
                state: target.state.clone(),
                progress: progress(record.batch.total_bytes, target.acknowledged_bytes),
                rate_label: None,
            })
            .collect(),
    }
}
fn progress(total: i64, acknowledged: i64) -> u8 {
    if total <= 0 {
        0
    } else {
        ((acknowledged.saturating_mul(100) / total).clamp(0, 100)) as u8
    }
}
fn history_from_record(record: &PersistedBatch) -> HistoryItemViewModel {
    HistoryItemViewModel {
        id: record.batch.id.clone(),
        direction: record.batch.direction.clone(),
        peer_name: record
            .targets
            .first()
            .map(|target| target.peer_device_id.clone())
            .unwrap_or_else(|| "Unknown device".into()),
        summary: format!(
            "{} item{}",
            record.items.len(),
            if record.items.len() == 1 { "" } else { "s" }
        ),
        time_label: record.batch.created_at.to_string(),
        state: record.batch.state.clone(),
        items: if record.batch.direction == "incoming" {
            record
                .items
                .iter()
                .filter(|item| item.parent_item_id.is_none())
                .map(|item| HistoryTopLevelItemViewModel {
                    item_id: item.id.clone(),
                    display_name: item.display_name.clone(),
                    kind: item.kind.clone(),
                    size: item.size,
                    state: item.state.clone(),
                    available: item
                        .destination_path_local
                        .as_deref()
                        .is_some_and(|path| std::path::Path::new(path).exists()),
                    destination_label: item
                        .destination_path_local
                        .as_deref()
                        .and_then(|path| std::path::Path::new(path).parent())
                        .map(|parent| parent.display().to_string()),
                })
                .collect()
        } else {
            Vec::new()
        },
    }
}
fn manifest_error(error: fileporter_transfer::TransferError) -> crate::error::AppError {
    use fileporter_transfer::TransferError;
    let (code, message) = match error {
        TransferError::SourceMissing => (
            "source_missing",
            "One or more selected files no longer exists.",
        ),
        TransferError::InvalidSource | TransferError::InvalidPath => (
            "invalid_path",
            "Selected paths must be absolute local files or folders.",
        ),
        TransferError::ManifestLimit => (
            "manifest_limit",
            "Selected files exceed Fileporter's supported limits.",
        ),
        TransferError::Durability => (
            "durability_failed",
            "Fileporter could not durably record transfer progress.",
        ),
        TransferError::DiskFull => ("disk_full", "The receive storage is full."),
        TransferError::FsyncFailed => (
            "fsync_failed",
            "Fileporter could not flush received data to storage.",
        ),
        TransferError::FinalizeFailed => (
            "finalize_failed",
            "Fileporter could not finalize the received item.",
        ),
        TransferError::Cancelled => ("cancelled", "Preparing the selected files was cancelled."),
    };
    crate::error::AppError::Validation {
        code,
        message,
        field: Some("paths"),
    }
}

pub fn validate_enqueue_request(
    request: &EnqueuePathsRequest,
) -> Result<(), crate::error::AppError> {
    if request.paths.is_empty() {
        return Err(crate::error::AppError::Validation {
            code: "invalid_path",
            message: "Choose at least one existing file or folder.",
            field: Some("paths"),
        });
    }
    if request.target_device_ids.is_empty() {
        return Err(crate::error::AppError::Validation {
            code: "no_targets",
            message: "Select at least one connected device before sending.",
            field: Some("targetDeviceIds"),
        });
    }
    if request.target_device_ids.len() > 64
        || request
            .target_device_ids
            .iter()
            .any(|id| id.trim().is_empty())
    {
        return Err(crate::error::AppError::Validation {
            code: "invalid_target",
            message: "One or more selected devices are invalid.",
            field: Some("targetDeviceIds"),
        });
    }
    if request.paths.len() > 256 || request.paths.iter().any(|path| path.trim().is_empty()) {
        return Err(crate::error::AppError::Validation {
            code: "invalid_path",
            message: "One or more selected paths are invalid.",
            field: Some("paths"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::TrustedPeer;
    use fileporter_network::TrustedPeerPin;
    use std::fs;

    fn peer_from_pin(_device_id: &str, pin: &TrustedPeerPin, endpoint: String) -> TrustedPeer {
        TrustedPeer {
            // TLS validates the certificate-derived device id, so test records
            // must retain the id carried by the pin rather than a display label.
            device_id: pin.device_id.clone(),
            public_key: pin.public_key.to_vec(),
            certificate_fingerprint: format!("blake3:{}", hex::encode(pin.certificate_fingerprint)),
            remote_name: pin.device_id.clone(),
            local_alias: None,
            paired_at: 1,
            last_seen_at: None,
            auto_send: false,
            revoked_at: None,
            endpoint: Some(endpoint),
        }
    }

    fn empty_pairing_snapshot() -> PairingSnapshot {
        PairingSnapshot {
            local_device_id: "local".into(),
            pending_pairings: Vec::new(),
            trusted_devices: Vec::new(),
        }
    }

    fn trusted_peer(device_id: &str) -> TrustedPeer {
        TrustedPeer {
            device_id: device_id.into(),
            public_key: vec![1],
            certificate_fingerprint: format!("sha256:{device_id}"),
            remote_name: "Test device".into(),
            local_alias: None,
            paired_at: 1,
            last_seen_at: None,
            auto_send: false,
            revoked_at: None,
            endpoint: Some("127.0.0.1:4242".into()),
        }
    }

    async fn connected_sender_and_receiver(directory: &std::path::Path) -> (AppState, Engine) {
        let sender =
            AppState::new(SettingsRepository::open(directory.join("sender.sqlite3")).unwrap());
        let receiver_repository =
            Arc::new(SettingsRepository::open(directory.join("receiver.sqlite3")).unwrap());
        let receive_directory = directory.join("received");
        fs::create_dir(&receive_directory).unwrap();
        receiver_repository
            .save(&Settings {
                device_name: "Receiver".into(),
                receive_directory: Some(receive_directory.to_string_lossy().into()),
                onboarding_complete: true,
                receiving_enabled: true,
                ..Settings::default()
            })
            .unwrap();
        let receiver_pairing = Arc::new(
            crate::identity::PairingCoordinator::open(receiver_repository.clone()).unwrap(),
        );
        let receiver = Engine::with_receiver(receiver_pairing.clone(), receiver_repository.clone());
        let endpoint = receiver
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let sender_pin = TrustedPeerPin::from_binding(sender.pairing.local_certificate().binding());
        receiver_repository
            .upsert_trusted_peer(&peer_from_pin("sender", &sender_pin, "127.0.0.1:1".into()))
            .unwrap();
        let receiver_pin =
            TrustedPeerPin::from_binding(receiver_pairing.local_certificate().binding());
        sender
            .settings
            .upsert_trusted_peer(&peer_from_pin(
                "receiver",
                &receiver_pin,
                endpoint.to_string(),
            ))
            .unwrap();
        (sender, receiver)
    }
    #[test]
    fn incomplete_onboarding_never_reports_managers_started() {
        assert!(
            !snapshot_from_settings(
                1,
                true,
                ListenerStatus {
                    listening: false,
                    receiving: false,
                    bound_endpoint: None,
                },
                false,
                Settings::default(),
                Vec::new(),
                Vec::new(),
                empty_pairing_snapshot()
            )
            .managers_started
        );
    }

    #[tokio::test]
    async fn persisted_receiving_preference_starts_and_stops_the_listener() {
        let directory = tempfile::tempdir().unwrap();
        let repository =
            SettingsRepository::open(directory.path().join("settings.sqlite3")).unwrap();
        let settings = Settings {
            onboarding_complete: true,
            receiving_enabled: true,
            listen_address: "127.0.0.1:0".into(),
            ..Settings::default()
        };
        repository.save(&settings).unwrap();
        let state = AppState::new(repository);

        state.reconcile_listener().await.unwrap();
        let running = state.snapshot(false).unwrap();
        assert!(running.lifecycle.listening);
        assert!(running.lifecycle.bound_endpoint.is_some());

        let mut disabled = state.settings.load().unwrap();
        disabled.receiving_enabled = false;
        state.settings.save(&disabled).unwrap();
        state.reconcile_listener().await.unwrap();
        let stopped = state.snapshot(false).unwrap();
        assert!(!stopped.lifecycle.listening);
        assert!(stopped.lifecycle.bound_endpoint.is_none());

        state.shutdown().await;
        assert!(state.shutdown_complete());
    }

    #[tokio::test]
    async fn sender_scheduler_starts_only_after_onboarding_and_shutdown_awaits_it() {
        let directory = tempfile::tempdir().unwrap();
        let repository =
            SettingsRepository::open(directory.path().join("settings.sqlite3")).unwrap();
        let state = AppState::new(repository);
        state.start_sender_scheduler();
        assert!(!state.scheduler.running.load(Ordering::Acquire));
        let mut settings = state.settings.load().unwrap();
        settings.onboarding_complete = true;
        state.settings.save(&settings).unwrap();
        state.start_sender_scheduler();
        assert!(state.scheduler.running.load(Ordering::Acquire));
        state.shutdown().await;
        assert!(!state.scheduler.running.load(Ordering::Acquire));
        assert!(state.shutdown_complete());
    }
    #[test]
    fn enqueue_requires_a_target() {
        assert!(validate_enqueue_request(&EnqueuePathsRequest {
            paths: vec!["C:/example.txt".into()],
            target_device_ids: Vec::new(),
            queue_offline: false
        })
        .is_err());
    }
    #[test]
    fn request_uses_the_frontend_camel_case_contract() {
        let request: EnqueuePathsRequest = serde_json::from_str(
            r#"{"paths":["C:/example.txt"],"targetDeviceIds":["peer-1"],"queueOffline":false}"#,
        )
        .unwrap();
        assert_eq!(request.target_device_ids, ["peer-1"]);
    }

    #[test]
    fn durable_enqueue_survives_reopening_and_hydrates_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("fileporter.sqlite3");
        let source = directory.path().join("notes.txt");
        fs::write(&source, b"notes").unwrap();
        {
            let repository = SettingsRepository::open(database.clone()).unwrap();
            repository
                .upsert_trusted_peer(&trusted_peer("peer-1"))
                .unwrap();
            let state = AppState::new(repository);
            let queued = state
                .queue_batch(EnqueuePathsRequest {
                    paths: vec![source.display().to_string()],
                    target_device_ids: vec!["peer-1".into()],
                    queue_offline: true,
                })
                .unwrap();
            assert_eq!(queued.item_count, 1);
            let persisted = state.settings.outgoing_batches().unwrap();
            assert_eq!(persisted.len(), 1);
            assert_eq!(persisted[0].items[0].size, 5);
            assert!(persisted[0].batch.wait_for_available);
            assert_eq!(
                persisted[0].items[0].source_path_local.as_deref(),
                Some(source.canonicalize().unwrap().to_string_lossy().as_ref())
            );
        }
        let state = AppState::new(SettingsRepository::open(database).unwrap());
        let snapshot = state.snapshot(true).unwrap();
        assert_eq!(snapshot.queued_batches.len(), 1);
        assert_eq!(snapshot.transfers.len(), 1);
        assert_eq!(snapshot.transfers[0].state, "queued");
        assert!(snapshot.history.is_empty());
        assert!(!snapshot.queued_batches[0].waiting_for_available);
    }

    #[test]
    fn offline_policy_persists_waiting_state_and_presence_wake_clears_backoff() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("db.sqlite3");
        let source = directory.path().join("notes.txt");
        fs::write(&source, b"notes").unwrap();
        let repository = SettingsRepository::open(database.clone()).unwrap();
        let mut peer = trusted_peer("peer-1");
        peer.endpoint = None;
        repository.upsert_trusted_peer(&peer).unwrap();
        let state = AppState::new(repository);
        let queued = state
            .queue_batch(EnqueuePathsRequest {
                paths: vec![source.display().to_string()],
                target_device_ids: vec!["peer-1".into()],
                queue_offline: true,
            })
            .unwrap();
        let record = state.settings.outgoing_batches().unwrap().pop().unwrap();
        assert!(state.defer_unavailable_targets(&record).unwrap());
        drop(state);
        let state = AppState::new(SettingsRepository::open(database).unwrap());
        let record = state.settings.outgoing_batches().unwrap().pop().unwrap();
        assert_eq!(record.targets[0].state, "waiting");
        assert!(record.targets[0].retry_at.is_some());
        // This is the same callback used after a trusted, pin-matched mDNS
        // presence observation; it wakes without waiting for the deadline.
        state.wake_waiting_peer("peer-1").unwrap();
        let target = state
            .settings
            .batch_targets(&queued.id)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(target.state, "queued");
        assert_eq!(target.retry_at, None);
    }

    #[test]
    fn cancellation_and_revocation_stop_waiting_targets() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("notes.txt");
        fs::write(&source, b"notes").unwrap();
        let repository = SettingsRepository::open(directory.path().join("db.sqlite3")).unwrap();
        repository
            .upsert_trusted_peer(&trusted_peer("peer-1"))
            .unwrap();
        let state = AppState::new(repository);
        let cancelled = state
            .queue_batch(EnqueuePathsRequest {
                paths: vec![source.display().to_string()],
                target_device_ids: vec!["peer-1".into()],
                queue_offline: true,
            })
            .unwrap();
        state.cancel_batch(&cancelled.id).unwrap();
        assert_eq!(
            state.settings.batch_targets(&cancelled.id).unwrap()[0].state,
            "cancelled"
        );
        let revoked = state
            .queue_batch(EnqueuePathsRequest {
                paths: vec![source.display().to_string()],
                target_device_ids: vec!["peer-1".into()],
                queue_offline: true,
            })
            .unwrap();
        state.fail_targets_for_revoked_peer("peer-1").unwrap();
        let target = state
            .settings
            .batch_targets(&revoked.id)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(target.state, "failed");
        assert_eq!(target.error_code.as_deref(), Some("recipient_revoked"));
    }

    #[test]
    fn durable_enqueue_rejects_unknown_recipient() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("notes.txt");
        fs::write(&source, b"notes").unwrap();
        let state =
            AppState::new(SettingsRepository::open(directory.path().join("db.sqlite3")).unwrap());
        let error = state
            .queue_batch(EnqueuePathsRequest {
                paths: vec![source.display().to_string()],
                target_device_ids: vec!["unknown".into()],
                queue_offline: false,
            })
            .unwrap_err();
        assert!(matches!(
            error,
            crate::error::AppError::Validation {
                code: "unknown_recipient",
                ..
            }
        ));
        assert!(state.settings.outgoing_batches().unwrap().is_empty());
    }

    #[test]
    fn durable_enqueue_rejects_missing_source() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("db.sqlite3");
        let repository = SettingsRepository::open(database.clone()).unwrap();
        repository
            .upsert_trusted_peer(&trusted_peer("peer-1"))
            .unwrap();
        let state = AppState::new(repository);
        let error = state
            .queue_batch(EnqueuePathsRequest {
                paths: vec![directory.path().join("gone.txt").display().to_string()],
                target_device_ids: vec!["peer-1".into()],
                queue_offline: false,
            })
            .unwrap_err();
        assert!(matches!(
            error,
            crate::error::AppError::Validation {
                code: "source_missing",
                ..
            }
        ));
        assert!(state.settings.outgoing_batches().unwrap().is_empty());
    }

    #[test]
    fn durable_enqueue_rejects_revoked_recipient_without_writing_a_batch() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("notes.txt");
        fs::write(&source, b"notes").unwrap();
        let database = directory.path().join("db.sqlite3");
        let repository = SettingsRepository::open(database.clone()).unwrap();
        let mut peer = trusted_peer("peer-1");
        peer.revoked_at = Some(2);
        repository.upsert_trusted_peer(&peer).unwrap();
        let state = AppState::new(repository);
        let error = state
            .queue_batch(EnqueuePathsRequest {
                paths: vec![source.display().to_string()],
                target_device_ids: vec!["peer-1".into()],
                queue_offline: false,
            })
            .unwrap_err();
        assert!(matches!(
            error,
            crate::error::AppError::Validation {
                code: "unknown_recipient",
                ..
            }
        ));
        assert!(state.settings.outgoing_batches().unwrap().is_empty());
    }

    #[test]
    fn cancel_and_retry_are_durable_and_never_reopen_completed_batches() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("notes.txt");
        fs::write(&source, b"notes").unwrap();
        let repository = SettingsRepository::open(directory.path().join("db.sqlite3")).unwrap();
        repository
            .upsert_trusted_peer(&trusted_peer("peer-1"))
            .unwrap();
        let state = AppState::new(repository);
        let queued = state
            .queue_batch(EnqueuePathsRequest {
                paths: vec![source.display().to_string()],
                target_device_ids: vec!["peer-1".into()],
                queue_offline: false,
            })
            .unwrap();
        state.cancel_batch(&queued.id).unwrap();
        assert_eq!(
            state.settings.batch(&queued.id).unwrap().unwrap().state,
            "cancelled"
        );
        state.retry_batch(&queued.id).unwrap();
        assert_eq!(
            state.settings.batch(&queued.id).unwrap().unwrap().state,
            "queued"
        );
        let mut completed = state.settings.batch(&queued.id).unwrap().unwrap();
        completed.state = "completed".into();
        completed.completed_at = Some(1);
        state.settings.save_batch(&completed).unwrap();
        state.cancel_batch(&queued.id).unwrap();
        assert_eq!(
            state.settings.batch(&queued.id).unwrap().unwrap().state,
            "completed"
        );
    }

    #[test]
    fn protocol_ids_are_stable_per_durable_record() {
        assert_eq!(stable_protocol_id("batch-1"), stable_protocol_id("batch-1"));
        assert_ne!(stable_protocol_id("batch-1"), stable_protocol_id("batch-2"));
    }

    #[tokio::test]
    async fn durable_sender_rejects_invalid_persisted_pin_without_connecting() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("notes.txt");
        fs::write(&source, b"notes").unwrap();
        let repository = SettingsRepository::open(directory.path().join("db.sqlite3")).unwrap();
        repository
            .upsert_trusted_peer(&trusted_peer("peer-1"))
            .unwrap();
        let state = AppState::new(repository);
        let queued = state
            .queue_batch(EnqueuePathsRequest {
                paths: vec![source.display().to_string()],
                target_device_ids: vec!["peer-1".into()],
                queue_offline: false,
            })
            .unwrap();
        assert!(matches!(
            state
                .send_queued_file(&queued.id, CancellationToken::new())
                .await,
            Err(crate::error::AppError::Validation {
                code: "invalid_peer_pin",
                ..
            })
        ));
    }

    #[tokio::test]
    async fn durable_sender_rechecks_revocation_after_queueing_without_checkpoint() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("notes.txt");
        fs::write(&source, b"notes").unwrap();
        let repository = SettingsRepository::open(directory.path().join("db.sqlite3")).unwrap();
        repository
            .upsert_trusted_peer(&trusted_peer("peer-1"))
            .unwrap();
        let state = AppState::new(repository);
        let queued = state
            .queue_batch(EnqueuePathsRequest {
                paths: vec![source.display().to_string()],
                target_device_ids: vec!["peer-1".into()],
                queue_offline: false,
            })
            .unwrap();
        state.settings.revoke_trusted_peer("peer-1", 2).unwrap();
        assert!(matches!(
            state
                .send_queued_file(&queued.id, CancellationToken::new())
                .await,
            Err(crate::error::AppError::Validation {
                code: "unknown_recipient",
                ..
            })
        ));
        let record = state.settings.outgoing_batches().unwrap().pop().unwrap();
        assert!(state
            .settings
            .checkpoint(&record.targets[0].id, &record.items[0].id)
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn durable_sender_rejects_public_persisted_endpoint() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("notes.txt");
        fs::write(&source, b"notes").unwrap();
        let database = directory.path().join("db.sqlite3");
        let repository = SettingsRepository::open(database.clone()).unwrap();
        let peer = trusted_peer("peer-1");
        repository.upsert_trusted_peer(&peer).unwrap();
        let state = AppState::new(repository);
        let queued = state
            .queue_batch(EnqueuePathsRequest {
                paths: vec![source.display().to_string()],
                target_device_ids: vec!["peer-1".into()],
                queue_offline: false,
            })
            .unwrap();
        drop(state);
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute(
                "UPDATE trusted_peers SET endpoint = '8.8.8.8:4242' WHERE device_id = 'peer-1'",
                [],
            )
            .unwrap();
        drop(connection);
        let state = AppState::new(SettingsRepository::open(database).unwrap());
        assert!(matches!(
            state
                .send_queued_file(&queued.id, CancellationToken::new())
                .await,
            Err(crate::error::AppError::Validation {
                code: "invalid_endpoint",
                ..
            })
        ));
    }

    #[tokio::test]
    async fn scheduler_batch_expands_multiple_picker_files_and_directory() {
        let directory = tempfile::tempdir().unwrap();
        let standalone = directory.path().join("standalone.txt");
        let folder = directory.path().join("Photos");
        fs::create_dir(&folder).unwrap();
        fs::write(&standalone, b"standalone").unwrap();
        fs::write(folder.join("first.txt"), b"first").unwrap();
        fs::write(folder.join("second.txt"), b"second").unwrap();
        let (state, receiver) = connected_sender_and_receiver(directory.path()).await;
        let receiver_id = state.settings.active_trusted_peers().unwrap()[0]
            .device_id
            .clone();
        let queued = state
            .queue_batch(EnqueuePathsRequest {
                paths: vec![
                    standalone.to_string_lossy().into(),
                    folder.to_string_lossy().into(),
                ],
                target_device_ids: vec![receiver_id],
                queue_offline: false,
            })
            .unwrap();
        assert_eq!(queued.item_count, 4);
        state
            .send_queued_file(&queued.id, CancellationToken::new())
            .await
            .unwrap();
        let record = state.settings.outgoing_batches().unwrap().pop().unwrap();
        assert_eq!(record.batch.state, "completed");
        assert_eq!(record.targets[0].state, "completed");
        assert_eq!(
            fs::read(directory.path().join("received/standalone.txt")).unwrap(),
            b"standalone"
        );
        assert_eq!(
            fs::read(directory.path().join("received/Photos/first.txt")).unwrap(),
            b"first"
        );
        assert_eq!(
            fs::read(directory.path().join("received/Photos/second.txt")).unwrap(),
            b"second"
        );
        receiver.shutdown_listener().await;
    }

    #[tokio::test]
    async fn scheduler_streams_large_file_with_bounded_source_reads() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("large.bin");
        fs::write(
            &source,
            vec![9u8; fileporter_protocol::MAX_CHUNK_DATA * 3 + 17],
        )
        .unwrap();
        let (state, receiver) = connected_sender_and_receiver(directory.path()).await;
        let receiver_id = state.settings.active_trusted_peers().unwrap()[0]
            .device_id
            .clone();
        let queued = state
            .queue_batch(EnqueuePathsRequest {
                paths: vec![source.to_string_lossy().into()],
                target_device_ids: vec![receiver_id],
                queue_offline: false,
            })
            .unwrap();
        crate::engine::reset_source_read_max_for_test();
        state
            .send_queued_file(&queued.id, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            crate::engine::source_read_max_for_test(),
            fileporter_protocol::MAX_CHUNK_DATA
        );
        assert_eq!(
            fs::metadata(directory.path().join("received/large.bin"))
                .unwrap()
                .len(),
            (fileporter_protocol::MAX_CHUNK_DATA * 3 + 17) as u64
        );
        receiver.shutdown_listener().await;
    }

    #[tokio::test]
    async fn scheduler_fails_source_mutated_after_enqueue_before_transfer() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("mutated.txt");
        fs::write(&source, b"original").unwrap();
        let (state, receiver) = connected_sender_and_receiver(directory.path()).await;
        let receiver_id = state.settings.active_trusted_peers().unwrap()[0]
            .device_id
            .clone();
        let queued = state
            .queue_batch(EnqueuePathsRequest {
                paths: vec![source.to_string_lossy().into()],
                target_device_ids: vec![receiver_id],
                queue_offline: false,
            })
            .unwrap();
        fs::write(&source, b"replacement content has a different length").unwrap();
        assert!(state
            .send_queued_file(&queued.id, CancellationToken::new())
            .await
            .is_err());
        let record = state.settings.outgoing_batches().unwrap().pop().unwrap();
        assert_eq!(record.batch.state, "failed");
        assert_eq!(record.targets[0].state, "failed");
        assert!(!directory.path().join("received/mutated.txt").exists());
        receiver.shutdown_listener().await;
    }

    #[tokio::test]
    async fn scheduler_checkpoint_write_failure_prevents_progress_and_completion() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("checkpoint.bin");
        fs::write(&source, vec![3u8; fileporter_protocol::MAX_CHUNK_DATA + 1]).unwrap();
        let (state, receiver) = connected_sender_and_receiver(directory.path()).await;
        let receiver_id = state.settings.active_trusted_peers().unwrap()[0]
            .device_id
            .clone();
        let queued = state
            .queue_batch(EnqueuePathsRequest {
                paths: vec![source.to_string_lossy().into()],
                target_device_ids: vec![receiver_id],
                queue_offline: false,
            })
            .unwrap();
        state.settings.fail_checkpoint_writes_for_test(true);
        assert!(state
            .send_queued_file(&queued.id, CancellationToken::new())
            .await
            .is_err());
        let record = state.settings.outgoing_batches().unwrap().pop().unwrap();
        assert_eq!(record.batch.state, "failed");
        assert_eq!(record.targets[0].state, "failed");
        assert_eq!(record.targets[0].acknowledged_bytes, 0);
        assert!(state
            .settings
            .checkpoint(&record.targets[0].id, &record.items[0].id)
            .unwrap()
            .is_none());
        receiver.shutdown_listener().await;
    }

    #[tokio::test]
    async fn fanout_keeps_successful_sibling_completed_when_other_target_fails() {
        let directory = tempfile::tempdir().unwrap();
        let sender_repository =
            SettingsRepository::open(directory.path().join("sender.sqlite3")).unwrap();
        sender_repository
            .save(&Settings {
                onboarding_complete: true,
                ..Settings::default()
            })
            .unwrap();
        let state = AppState::new(sender_repository);
        let receiver_repository =
            Arc::new(SettingsRepository::open(directory.path().join("receiver.sqlite3")).unwrap());
        receiver_repository
            .save(&Settings {
                device_name: "Receiver".into(),
                receive_directory: Some(directory.path().to_string_lossy().into()),
                onboarding_complete: true,
                receiving_enabled: true,
                ..Settings::default()
            })
            .unwrap();
        let receiver_pairing = Arc::new(
            crate::identity::PairingCoordinator::open(receiver_repository.clone()).unwrap(),
        );
        let receiver = crate::engine::Engine::with_receiver(
            receiver_pairing.clone(),
            receiver_repository.clone(),
        );
        let endpoint = receiver
            .start_listener("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let sender_pin = TrustedPeerPin::from_binding(state.pairing.local_certificate().binding());
        receiver_repository
            .upsert_trusted_peer(&peer_from_pin("sender", &sender_pin, "127.0.0.1:1".into()))
            .unwrap();
        let receiver_pin =
            TrustedPeerPin::from_binding(receiver_pairing.local_certificate().binding());
        state
            .settings
            .upsert_trusted_peer(&peer_from_pin(
                "success",
                &receiver_pin,
                endpoint.to_string(),
            ))
            .unwrap();
        let success_id = receiver_pin.device_id.clone();
        // Keep the second target cryptographically valid but unreachable.  This
        // exercises the production connection-failure path rather than failing
        // early while parsing an intentionally bogus pin.
        let offline_repository =
            Arc::new(SettingsRepository::open(directory.path().join("offline.sqlite3")).unwrap());
        let offline_pairing =
            crate::identity::PairingCoordinator::open(offline_repository).unwrap();
        let offline_pin =
            TrustedPeerPin::from_binding(offline_pairing.local_certificate().binding());
        let offline_id = offline_pin.device_id.clone();
        state
            .settings
            .upsert_trusted_peer(&peer_from_pin(
                "offline",
                &offline_pin,
                "127.0.0.1:9".into(),
            ))
            .unwrap();
        let source = directory.path().join("fanout.txt");
        fs::write(&source, b"fanout").unwrap();
        let queued = state
            .queue_batch(EnqueuePathsRequest {
                paths: vec![source.to_string_lossy().into()],
                target_device_ids: vec![success_id.clone(), offline_id.clone()],
                queue_offline: false,
            })
            .unwrap();
        assert!(state
            .send_queued_file(&queued.id, CancellationToken::new())
            .await
            .is_err());
        let record = state.settings.outgoing_batches().unwrap().pop().unwrap();
        assert_eq!(record.batch.state, "failed");
        assert_eq!(
            record
                .targets
                .iter()
                .find(|target| target.peer_device_id == success_id)
                .unwrap()
                .state,
            "completed"
        );
        assert_eq!(
            record
                .targets
                .iter()
                .find(|target| target.peer_device_id == offline_id)
                .unwrap()
                .state,
            "failed"
        );
        assert_eq!(
            fs::read(directory.path().join("fanout.txt")).unwrap(),
            b"fanout"
        );
        receiver.shutdown_listener().await;
        state.shutdown().await;
    }

    #[tokio::test]
    async fn suspend_resume_and_reconcile_are_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let repository =
            SettingsRepository::open(directory.path().join("lifecycle.sqlite")).unwrap();
        let mut settings = repository.load().unwrap();
        settings.onboarding_complete = true;
        settings.receiving_enabled = true;
        settings.listen_address = "127.0.0.1:0".into();
        repository.save(&settings).unwrap();
        let state = AppState::new(repository);
        state.reconcile_listener().await.unwrap();
        state.reconcile_listener().await.unwrap();
        assert!(state.engine.listener_status().listening);
        state.suspend().await;
        state.suspend().await;
        assert!(!state.engine.listener_status().listening);
        state.resume_or_network_changed().await.unwrap();
        state.resume_or_network_changed().await.unwrap();
        assert!(state.engine.listener_status().listening);
        assert!(state.scheduler.running.load(Ordering::Acquire));
        state.shutdown().await;
    }

    #[test]
    fn diagnostics_helpers_are_bounded_and_path_free() {
        let directory = tempfile::tempdir().unwrap();
        let staging = directory.path().join(".fileporter-staging").join("batch");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("chunk"), b"1234").unwrap();
        assert_eq!(owned_staging_bytes(directory.path().to_str()), 4);
        let batches = (0..20)
            .map(|index| PersistedBatch {
                batch: Batch {
                    id: index.to_string(),
                    direction: "outgoing".into(),
                    state: "failed".into(),
                    created_at: 0,
                    completed_at: None,
                    total_bytes: 0,
                    total_entries: 0,
                    warning_count: 0,
                    wait_for_available: false,
                },
                targets: vec![BatchTarget {
                    id: index.to_string(),
                    batch_id: index.to_string(),
                    peer_device_id: "peer".into(),
                    state: "failed".into(),
                    acknowledged_bytes: 0,
                    error_code: Some(format!("error_{index}")),
                    retry_at: None,
                    retry_count: 0,
                    wait_for_available: false,
                }],
                items: Vec::new(),
            })
            .collect::<Vec<_>>();
        let codes = recent_error_codes(&batches);
        assert_eq!(codes.len(), 16);
        assert!(codes
            .iter()
            .all(|code| !code.contains('\\') && !code.contains('/')));
    }
}
