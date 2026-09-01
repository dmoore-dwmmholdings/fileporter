//! LAN discovery is deliberately an unauthenticated *hint* channel.
//!
//! The adapter boundary keeps multicast out of unit tests. A discovery record
//! can trigger the limited authenticated pairing channel, but it never grants
//! transfer trust by itself. Online presence still requires an exact durable
//! device-id and certificate-pin match.

use std::{
    collections::{HashMap, VecDeque},
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    sync::Mutex,
};

use flume::Receiver;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::{
    engine::{is_loopback_or_private, validate_manual_endpoint},
    persistence::{SettingsRepository, TrustedPeer},
};

pub const SERVICE_TYPE: &str = "_fileporter._tcp.local.";
pub const PROTOCOL_VERSION: u16 = 1;
pub const CAPABILITIES: &[&str] = &["receive-v1", "pairing-v1"];
pub const PRESENCE_TTL_SECS: i64 = 90;
const MAX_NEARBY_CANDIDATES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRecord {
    pub device_id: String,
    pub device_name: String,
    pub endpoint: SocketAddr,
    /// BLAKE3 certificate fingerprint, not a certificate, public key, or secret.
    pub certificate_fingerprint: String,
    pub protocol_version: u16,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
    pub device_id: String,
    pub online: bool,
    pub last_seen_at: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearbyCandidate {
    pub record: DiscoveryRecord,
    pub last_seen_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdnsState {
    Disabled,
    Starting,
    Advertising,
    Browsing,
    Error,
}
impl MdnsState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Starting => "starting",
            Self::Advertising => "advertising",
            Self::Browsing => "browsing",
            Self::Error => "error",
        }
    }
}

pub trait DiscoveryAdapter: Send {
    fn publish(&mut self, record: &DiscoveryRecord) -> Result<(), String>;
    fn withdraw(&mut self) -> Result<(), String>;
    fn browse(&mut self) -> Result<Vec<DiscoveryRecord>, String>;
}

/// Production wiring point for the platform mDNS implementation. It is kept
/// separate from policy so tests (and headless builds) need no multicast.
pub struct MdnsDiscoveryAdapter {
    daemon: Option<ServiceDaemon>,
    events: Option<Receiver<ServiceEvent>>,
    fullname: Option<String>,
}
impl MdnsDiscoveryAdapter {
    pub fn new() -> Self {
        Self {
            daemon: None,
            events: None,
            fullname: None,
        }
    }
    fn ensure_running(&mut self) -> Result<(), String> {
        if self.daemon.is_some() && self.events.is_some() {
            return Ok(());
        }
        let daemon = ServiceDaemon::new().map_err(|error| error.to_string())?;
        let events = daemon
            .browse(SERVICE_TYPE)
            .map_err(|error| error.to_string())?;
        self.daemon = Some(daemon);
        self.events = Some(events);
        Ok(())
    }
    fn reset(&mut self) {
        self.daemon = None;
        self.events = None;
        self.fullname = None;
    }
}
impl DiscoveryAdapter for MdnsDiscoveryAdapter {
    fn publish(&mut self, record: &DiscoveryRecord) -> Result<(), String> {
        self.ensure_running()?;
        let properties = [
            ("id", record.device_id.as_str()),
            ("name", record.device_name.as_str()),
            ("pin", record.certificate_fingerprint.as_str()),
            ("ver", &record.protocol_version.to_string()),
            ("caps", &record.capabilities.join(",")),
        ];
        let instance = format!("fileporter-{}", record.device_id);
        let host = format!("{}.local.", instance);
        let info = if record.endpoint.ip().is_unspecified() {
            ServiceInfo::new(
                SERVICE_TYPE,
                &instance,
                &host,
                (),
                record.endpoint.port(),
                &properties[..],
            )
            .map_err(|error| error.to_string())?
            .enable_addr_auto()
        } else {
            ServiceInfo::new(
                SERVICE_TYPE,
                &instance,
                &host,
                record.endpoint.ip(),
                record.endpoint.port(),
                &properties[..],
            )
            .map_err(|error| error.to_string())?
        };
        self.fullname = Some(info.get_fullname().to_owned());
        if let Err(error) = self
            .daemon
            .as_ref()
            .expect("mDNS daemon initialized")
            .register(info)
        {
            self.reset();
            return Err(error.to_string());
        }
        Ok(())
    }
    fn withdraw(&mut self) -> Result<(), String> {
        if let Some(fullname) = self.fullname.take() {
            if let Some(daemon) = self.daemon.as_ref() {
                if let Err(error) = daemon.unregister(&fullname) {
                    self.reset();
                    return Err(error.to_string());
                }
            }
        }
        Ok(())
    }
    fn browse(&mut self) -> Result<Vec<DiscoveryRecord>, String> {
        if self.ensure_running().is_err() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        while let Ok(event) = self
            .events
            .as_ref()
            .expect("mDNS events initialized")
            .try_recv()
        {
            let ServiceEvent::ServiceResolved(info) = event else {
                continue;
            };
            let Some(endpoint) = preferred_lan_endpoint(info.get_addresses(), info.get_port())
            else {
                continue;
            };
            let Some(device_id) = info.get_property_val_str("id") else {
                continue;
            };
            let Some(device_name) = info.get_property_val_str("name") else {
                continue;
            };
            let Some(certificate_fingerprint) = info.get_property_val_str("pin") else {
                continue;
            };
            let Ok(protocol_version) = info.get_property_val_str("ver").unwrap_or_default().parse()
            else {
                continue;
            };
            let capabilities = info
                .get_property_val_str("caps")
                .unwrap_or_default()
                .split(',')
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect();
            records.push(DiscoveryRecord {
                device_id: device_id.to_owned(),
                device_name: device_name.to_owned(),
                endpoint,
                certificate_fingerprint: certificate_fingerprint.to_owned(),
                protocol_version,
                capabilities,
            });
        }
        Ok(records)
    }
}

fn preferred_lan_endpoint(
    addresses: &std::collections::HashSet<IpAddr>,
    port: u16,
) -> Option<SocketAddr> {
    preferred_lan_endpoint_for_local(addresses, port, primary_lan_ip())
}

fn preferred_lan_endpoint_for_local(
    addresses: &std::collections::HashSet<IpAddr>,
    port: u16,
    local: Option<IpAddr>,
) -> Option<SocketAddr> {
    if port == 0 {
        return None;
    }
    addresses
        .iter()
        .copied()
        .filter(|address| is_loopback_or_private(*address))
        .min_by_key(|address| match address {
            IpAddr::V4(address) if address.is_private() => {
                let distance = match local {
                    Some(IpAddr::V4(local)) => u32::from(*address) ^ u32::from(local),
                    _ => u32::MAX,
                };
                (0, u128::from(distance))
            }
            IpAddr::V6(address) => {
                let distance = match local {
                    Some(IpAddr::V6(local)) => u128::from(*address) ^ u128::from(local),
                    _ => u128::MAX,
                };
                (1, distance)
            }
            IpAddr::V4(_) => (2, u128::MAX),
        })
        .map(|address| SocketAddr::new(address, port))
}

/// Turns an all-interface bind address into the concrete address peers should
/// dial. UDP connect only asks the OS routing table for the selected interface;
/// it sends no probe packet.
pub fn resolve_advertised_endpoint(bound: SocketAddr) -> Option<SocketAddr> {
    if !bound.ip().is_unspecified() {
        return is_loopback_or_private(bound.ip()).then_some(bound);
    }
    primary_lan_ip().map(|address| SocketAddr::new(address, bound.port()))
}

fn primary_lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    let address = socket.local_addr().ok()?.ip();
    match address {
        IpAddr::V4(address) if address.is_private() => Some(IpAddr::V4(address)),
        _ => None,
    }
}

pub struct DiscoveryCoordinator {
    adapter: Mutex<Box<dyn DiscoveryAdapter>>,
    advertised: Mutex<Option<DiscoveryRecord>>,
    presence: Mutex<HashMap<String, Presence>>,
    nearby: Mutex<HashMap<String, NearbyCandidate>>,
    state: Mutex<MdnsState>,
    errors: Mutex<VecDeque<String>>,
}

impl DiscoveryCoordinator {
    pub fn new(adapter: Box<dyn DiscoveryAdapter>) -> Self {
        Self {
            adapter: Mutex::new(adapter),
            advertised: Mutex::new(None),
            presence: Mutex::new(HashMap::new()),
            nearby: Mutex::new(HashMap::new()),
            state: Mutex::new(MdnsState::Disabled),
            errors: Mutex::new(VecDeque::new()),
        }
    }

    /// Advertise only the public identity, compatibility metadata, and bound
    /// endpoint. A listener restart naturally replaces the published record.
    pub fn reconcile(
        &self,
        local: Option<DiscoveryRecord>,
        repository: &SettingsRepository,
        now: i64,
    ) -> Vec<String> {
        let mut adapter = self
            .adapter
            .lock()
            .expect("discovery adapter mutex poisoned");
        let mut advertised = self
            .advertised
            .lock()
            .expect("discovery advertisement mutex poisoned");
        *self.state.lock().expect("mDNS state mutex poisoned") = if local.is_some() {
            MdnsState::Starting
        } else {
            MdnsState::Disabled
        };
        if *advertised != local {
            if advertised.is_some() && adapter.withdraw().is_err() {
                *self.state.lock().expect("mDNS state mutex poisoned") = MdnsState::Error;
                self.record_error("mdns_withdraw_failed");
            }
            if let Some(record) = local.clone() {
                if adapter.publish(&record).is_err() {
                    *self.state.lock().expect("mDNS state mutex poisoned") = MdnsState::Error;
                    self.record_error("mdns_publish_failed");
                }
            }
            *advertised = local;
        }
        let records = match adapter.browse() {
            Ok(records) => {
                let mut state = self.state.lock().expect("mDNS state mutex poisoned");
                if *state != MdnsState::Error {
                    *state = if advertised.is_some() {
                        MdnsState::Advertising
                    } else {
                        MdnsState::Browsing
                    };
                }
                records
            }
            Err(_) => {
                *self.state.lock().expect("mDNS state mutex poisoned") = MdnsState::Error;
                self.record_error("mdns_browse_failed");
                Vec::new()
            }
        };
        drop(advertised);
        drop(adapter);
        self.observe_records(records, repository, now)
    }

    /// Poll browsing without altering the local advertisement. This is used by
    /// the sender lifecycle so a trusted peer coming online wakes queued work.
    pub fn refresh(&self, repository: &SettingsRepository, now: i64) -> Vec<String> {
        let records = match self
            .adapter
            .lock()
            .expect("discovery adapter mutex poisoned")
            .browse()
        {
            Ok(records) => {
                let mut state = self.state.lock().expect("mDNS state mutex poisoned");
                if *state != MdnsState::Error {
                    *state = MdnsState::Browsing;
                }
                records
            }
            Err(_) => {
                *self.state.lock().expect("mDNS state mutex poisoned") = MdnsState::Error;
                self.record_error("mdns_browse_failed");
                Vec::new()
            }
        };
        self.observe_records(records, repository, now)
    }

    fn observe_records(
        &self,
        mut records: Vec<DiscoveryRecord>,
        repository: &SettingsRepository,
        now: i64,
    ) -> Vec<String> {
        records.sort_by(|left, right| {
            left.device_id
                .cmp(&right.device_id)
                .then_with(|| left.endpoint.cmp(&right.endpoint))
        });
        let mut observed = Vec::new();
        for record in records {
            let device_id = record.device_id.clone();
            if self.observe(record, repository, now) {
                observed.push(device_id);
            }
        }
        self.expire(now);
        observed
    }

    fn observe(&self, record: DiscoveryRecord, repository: &SettingsRepository, now: i64) -> bool {
        if !valid_record(&record) {
            return false;
        }
        let Ok(Some(peer)) = repository.trusted_peer(&record.device_id) else {
            let mut nearby = self.nearby.lock().expect("nearby mutex poisoned");
            if let Some(existing) = nearby.get(&record.device_id) {
                // A competing fingerprint for the same id is a spoof claim;
                // retain the first fresh validated candidate until expiry.
                if normalize_pin(&existing.record.certificate_fingerprint)
                    != normalize_pin(&record.certificate_fingerprint)
                {
                    return false;
                }
            }
            if nearby.len() >= MAX_NEARBY_CANDIDATES && !nearby.contains_key(&record.device_id) {
                return false;
            }
            nearby.insert(
                record.device_id.clone(),
                NearbyCandidate {
                    record,
                    last_seen_at: now,
                },
            );
            return true;
        };
        // A revoked row is a durable deny decision, not a trusted presence.
        if peer.revoked_at.is_some() {
            self.nearby
                .lock()
                .expect("nearby mutex poisoned")
                .remove(&record.device_id);
            return false;
        }
        // Correlate the discovery claim with the durable identity/pin before
        // recording reachability or changing a usable endpoint.
        if normalize_pin(&peer.certificate_fingerprint)
            != normalize_pin(&record.certificate_fingerprint)
        {
            return false;
        }
        let mut updated: TrustedPeer = peer;
        updated.endpoint = Some(record.endpoint.to_string());
        updated.last_seen_at = Some(now);
        if repository.upsert_trusted_peer(&updated).is_err() {
            return false;
        }
        self.nearby
            .lock()
            .expect("nearby mutex poisoned")
            .remove(&record.device_id);
        self.presence
            .lock()
            .expect("presence mutex poisoned")
            .insert(
                record.device_id.clone(),
                Presence {
                    device_id: record.device_id,
                    online: true,
                    last_seen_at: Some(now),
                },
            );
        true
    }

    pub fn expire(&self, now: i64) {
        for presence in self
            .presence
            .lock()
            .expect("presence mutex poisoned")
            .values_mut()
        {
            if presence
                .last_seen_at
                .is_some_and(|seen| now.saturating_sub(seen) >= PRESENCE_TTL_SECS)
            {
                presence.online = false;
            }
        }
        self.nearby
            .lock()
            .expect("nearby mutex poisoned")
            .retain(|_, candidate| now.saturating_sub(candidate.last_seen_at) < PRESENCE_TTL_SECS);
    }
    pub fn snapshot(&self, peer: &TrustedPeer) -> Presence {
        self.presence
            .lock()
            .expect("presence mutex poisoned")
            .get(&peer.device_id)
            .cloned()
            .unwrap_or(Presence {
                device_id: peer.device_id.clone(),
                online: false,
                last_seen_at: peer.last_seen_at,
            })
    }
    pub fn nearby(&self) -> Vec<NearbyCandidate> {
        let mut values: Vec<_> = self
            .nearby
            .lock()
            .expect("nearby mutex poisoned")
            .values()
            .cloned()
            .collect();
        values.sort_by(|a, b| a.record.device_id.cmp(&b.record.device_id));
        values
    }
    pub fn mdns_state(&self) -> MdnsState {
        *self.state.lock().expect("mDNS state mutex poisoned")
    }
    pub fn recent_error_codes(&self) -> Vec<String> {
        self.errors
            .lock()
            .expect("mDNS error mutex poisoned")
            .iter()
            .cloned()
            .collect()
    }
    fn record_error(&self, code: &'static str) {
        let mut errors = self.errors.lock().expect("mDNS error mutex poisoned");
        if errors.back().is_some_and(|latest| latest == code) {
            return;
        }
        errors.push_back(code.into());
        if errors.len() > 16 {
            errors.pop_front();
        }
    }
    #[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Used by the desktop discovered-pairing command.
    pub fn candidate(&self, id: &str) -> Option<NearbyCandidate> {
        self.nearby
            .lock()
            .expect("nearby mutex poisoned")
            .get(id)
            .cloned()
    }

    /// Re-correlates a retained discovery candidate after an authenticated
    /// trust exchange commits, promoting it to online presence only on an
    /// exact durable certificate-pin match.
    pub fn promote_trusted(&self, id: &str, repository: &SettingsRepository, now: i64) -> bool {
        let Some(candidate) = self.candidate(id) else {
            return false;
        };
        self.observe(candidate.record, repository, now)
    }
}

fn valid_record(record: &DiscoveryRecord) -> bool {
    record.protocol_version == PROTOCOL_VERSION
        && !record.device_id.is_empty()
        && record.device_id.len() <= 128
        && !record.device_name.trim().is_empty()
        && record.device_name.len() <= 128
        && record
            .capabilities
            .iter()
            .all(|capability| CAPABILITIES.contains(&capability.as_str()))
        && record
            .capabilities
            .iter()
            .any(|capability| capability == "receive-v1")
        && normalize_pin(&record.certificate_fingerprint).len() == 64
        && validate_manual_endpoint(&record.endpoint.to_string()).is_ok()
}
fn normalize_pin(pin: &str) -> String {
    pin.trim()
        .strip_prefix("blake3:")
        .unwrap_or(pin.trim())
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::SettingsRepository;
    use std::{collections::VecDeque, sync::Arc};

    #[derive(Default)]
    struct Mock {
        published: Vec<DiscoveryRecord>,
        withdraws: usize,
        records: VecDeque<Vec<DiscoveryRecord>>,
    }
    impl DiscoveryAdapter for Arc<Mutex<Mock>> {
        fn publish(&mut self, record: &DiscoveryRecord) -> Result<(), String> {
            self.lock().unwrap().published.push(record.clone());
            Ok(())
        }
        fn withdraw(&mut self) -> Result<(), String> {
            self.lock().unwrap().withdraws += 1;
            Ok(())
        }
        fn browse(&mut self) -> Result<Vec<DiscoveryRecord>, String> {
            Ok(self.lock().unwrap().records.pop_front().unwrap_or_default())
        }
    }
    fn record(id: &str, pin: &str, endpoint: &str) -> DiscoveryRecord {
        DiscoveryRecord {
            device_id: id.into(),
            device_name: "Laptop".into(),
            endpoint: endpoint.parse().unwrap(),
            certificate_fingerprint: pin.into(),
            protocol_version: 1,
            capabilities: vec!["receive-v1".into()],
        }
    }
    fn peer(id: &str, pin: &str) -> TrustedPeer {
        TrustedPeer {
            device_id: id.into(),
            public_key: vec![1],
            certificate_fingerprint: pin.into(),
            remote_name: "Laptop".into(),
            local_alias: None,
            paired_at: 1,
            last_seen_at: None,
            auto_send: false,
            revoked_at: None,
            endpoint: None,
        }
    }
    #[test]
    fn resolved_service_prefers_a_reachable_private_ipv4_address() {
        let addresses = [
            "fe80::1234".parse().unwrap(),
            "fd00::1234".parse().unwrap(),
            "8.8.8.8".parse().unwrap(),
            "192.168.1.24".parse().unwrap(),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            preferred_lan_endpoint_for_local(
                &addresses,
                4242,
                Some("192.168.1.200".parse().unwrap())
            ),
            Some("192.168.1.24:4242".parse().unwrap())
        );
        assert_eq!(preferred_lan_endpoint_for_local(&addresses, 0, None), None);
        assert_eq!(
            resolve_advertised_endpoint("127.0.0.1:4242".parse().unwrap()),
            Some("127.0.0.1:4242".parse().unwrap())
        );
    }

    #[test]
    fn advertise_update_expiry_spoof_and_restart_are_deterministic() {
        let temp = tempfile::tempdir().unwrap();
        let repo = SettingsRepository::open(temp.path().join("db.sqlite")).unwrap();
        repo.upsert_trusted_peer(&peer("trusted", &"a1".repeat(32)))
            .unwrap();
        let mock = Arc::new(Mutex::new(Mock::default()));
        let discovery = DiscoveryCoordinator::new(Box::new(mock.clone()));
        let local = record("local", &"b2".repeat(32), "127.0.0.1:4242");
        discovery.reconcile(Some(local.clone()), &repo, 10);
        discovery.reconcile(
            Some(record("local", &"b2".repeat(32), "127.0.0.1:4243")),
            &repo,
            11,
        );
        mock.lock().unwrap().records.push_back(vec![
            record("spoof", &"a1".repeat(32), "127.0.0.1:4242"),
            record("trusted", &"ff".repeat(32), "127.0.0.1:4242"),
            record("trusted", &"a1".repeat(32), "8.8.8.8:4242"),
            record("trusted", &"a1".repeat(32), "127.0.0.1:4343"),
        ]);
        let observed = discovery.reconcile(
            Some(record("local", &"b2".repeat(32), "127.0.0.1:4243")),
            &repo,
            20,
        );
        assert_eq!(observed, ["spoof", "trusted"]);
        assert_eq!(discovery.nearby().len(), 1);
        assert_eq!(
            repo.trusted_peer("trusted")
                .unwrap()
                .unwrap()
                .endpoint
                .as_deref(),
            Some("127.0.0.1:4343")
        );
        assert!(
            discovery
                .snapshot(&repo.trusted_peer("trusted").unwrap().unwrap())
                .online
        );
        discovery.expire(20 + PRESENCE_TTL_SECS);
        assert!(
            !discovery
                .snapshot(&repo.trusted_peer("trusted").unwrap().unwrap())
                .online
        );
        discovery.reconcile(None, &repo, 111);
        let state = mock.lock().unwrap();
        assert_eq!(state.published.len(), 2);
        assert_eq!(state.withdraws, 2);
    }

    #[test]
    fn revoked_identity_is_neither_nearby_nor_online_and_cannot_be_promoted() {
        let temp = tempfile::tempdir().unwrap();
        let repo = SettingsRepository::open(temp.path().join("db.sqlite")).unwrap();
        let mut revoked = peer("forgotten", &"a1".repeat(32));
        revoked.revoked_at = Some(9);
        repo.upsert_trusted_peer(&revoked).unwrap();
        let mock = Arc::new(Mutex::new(Mock::default()));
        mock.lock().unwrap().records.push_back(vec![record(
            "forgotten",
            &"a1".repeat(32),
            "127.0.0.1:4242",
        )]);
        let discovery = DiscoveryCoordinator::new(Box::new(mock));

        assert!(discovery.refresh(&repo, 10).is_empty());
        assert!(discovery.nearby().is_empty());
        assert!(!discovery.promote_trusted("forgotten", &repo, 11));
        assert!(!discovery.snapshot(&revoked).online);
        assert!(repo
            .trusted_peer("forgotten")
            .unwrap()
            .unwrap()
            .endpoint
            .is_none());
    }
}
