//! Discovery through the system mDNSResponder (`dns_sd.h`).
//!
//! iOS refuses raw multicast sockets to apps without a restricted entitlement,
//! so the `mdns-sd` responder the desktop embeds cannot run there. The system
//! responder speaks the same `_fileporter._tcp` records on the wire and only
//! needs the Bonjour service declared in the app's Info.plist.
//!
//! `DNSServiceRef`s are not thread-safe. One worker thread owns every ref and
//! is driven by commands; the adapter only exchanges plain data with it.
#![cfg_attr(not(target_os = "ios"), allow(dead_code))]

use std::{
    collections::{HashMap, HashSet},
    ffi::{c_char, c_int, c_void, CStr, CString},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use crate::discovery::{DiscoveryAdapter, DiscoveryRecord};

const REGTYPE: &str = "_fileporter._tcp";
/// Automated UI tests advertise under a separate service so simulators never
/// link with real pads on the same network. Apps launched normally have no
/// such variable.
const REGTYPE_OVERRIDE: &str = "FILEPORTER_DNSSD_SERVICE";

fn regtype() -> String {
    std::env::var(REGTYPE_OVERRIDE)
        .ok()
        .filter(|value| value.starts_with('_') && value.ends_with("._tcp"))
        .unwrap_or_else(|| REGTYPE.to_owned())
}
/// Resolves are cheap and the responder caches them; repeating them is how an
/// endpoint that changed port after a restart is noticed.
const RERESOLVE_EVERY: Duration = Duration::from_secs(30);
/// Address lookups stay open briefly so both address families can answer.
const ADDRESS_WINDOW: Duration = Duration::from_secs(4);

type DnsServiceRef = *mut c_void;
type DnsServiceFlags = u32;
type DnsServiceError = i32;

const FLAGS_ADD: DnsServiceFlags = 0x2;
const PROTOCOL_IPV4: u32 = 0x1;
const PROTOCOL_IPV6: u32 = 0x2;
const AF_INET: u8 = 2;
const AF_INET6: u8 = 30;
const POLLIN: i16 = 0x1;

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}

type BrowseReply = extern "C" fn(
    DnsServiceRef,
    DnsServiceFlags,
    u32,
    DnsServiceError,
    *const c_char,
    *const c_char,
    *const c_char,
    *mut c_void,
);
type ResolveReply = extern "C" fn(
    DnsServiceRef,
    DnsServiceFlags,
    u32,
    DnsServiceError,
    *const c_char,
    *const c_char,
    u16,
    u16,
    *const u8,
    *mut c_void,
);
type AddrInfoReply = extern "C" fn(
    DnsServiceRef,
    DnsServiceFlags,
    u32,
    DnsServiceError,
    *const c_char,
    *const u8,
    u32,
    *mut c_void,
);
type RegisterReply = extern "C" fn(
    DnsServiceRef,
    DnsServiceFlags,
    DnsServiceError,
    *const c_char,
    *const c_char,
    *const c_char,
    *mut c_void,
);

extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn DNSServiceRegister(
        sd_ref: *mut DnsServiceRef,
        flags: DnsServiceFlags,
        interface_index: u32,
        name: *const c_char,
        regtype: *const c_char,
        domain: *const c_char,
        host: *const c_char,
        port: u16,
        txt_len: u16,
        txt_record: *const c_void,
        callback: RegisterReply,
        context: *mut c_void,
    ) -> DnsServiceError;
    fn DNSServiceBrowse(
        sd_ref: *mut DnsServiceRef,
        flags: DnsServiceFlags,
        interface_index: u32,
        regtype: *const c_char,
        domain: *const c_char,
        callback: BrowseReply,
        context: *mut c_void,
    ) -> DnsServiceError;
    fn DNSServiceResolve(
        sd_ref: *mut DnsServiceRef,
        flags: DnsServiceFlags,
        interface_index: u32,
        name: *const c_char,
        regtype: *const c_char,
        domain: *const c_char,
        callback: ResolveReply,
        context: *mut c_void,
    ) -> DnsServiceError;
    fn DNSServiceGetAddrInfo(
        sd_ref: *mut DnsServiceRef,
        flags: DnsServiceFlags,
        interface_index: u32,
        protocol: u32,
        hostname: *const c_char,
        callback: AddrInfoReply,
        context: *mut c_void,
    ) -> DnsServiceError;
    fn DNSServiceRefSockFD(sd_ref: DnsServiceRef) -> c_int;
    fn DNSServiceProcessResult(sd_ref: DnsServiceRef) -> DnsServiceError;
    fn DNSServiceRefDeallocate(sd_ref: DnsServiceRef);
    fn poll(fds: *mut PollFd, count: u32, timeout: c_int) -> c_int;
}

/// A service instance as browse reports it: enough to resolve it again.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Instance {
    name: String,
    domain: String,
    interface: u32,
}

#[derive(Default)]
struct EventQueue(Vec<Event>);

/// What callbacks report. They only record; the worker loop acts afterwards,
/// so no ref is ever deallocated from inside its own callback.
enum Event {
    Found(Instance),
    Lost(Instance),
    Resolved {
        instance: Instance,
        host: String,
        port: u16,
        txt: Vec<u8>,
    },
    Address {
        host: String,
        interface: u32,
        address: IpAddr,
    },
    Failed(&'static str),
}

enum Command {
    Publish(DiscoveryRecord),
    Withdraw,
}

#[derive(Default)]
struct Shared {
    records: Vec<DiscoveryRecord>,
    departed: Vec<String>,
    errors: Vec<&'static str>,
}

pub struct DnsSdDiscoveryAdapter {
    commands: Option<mpsc::Sender<Command>>,
    shared: Arc<Mutex<Shared>>,
}

impl DnsSdDiscoveryAdapter {
    pub fn new() -> Self {
        Self {
            commands: None,
            shared: Arc::new(Mutex::new(Shared::default())),
        }
    }

    fn ensure_running(&mut self) -> Result<&mpsc::Sender<Command>, String> {
        if self.commands.is_none() {
            let (tx, rx) = mpsc::channel();
            let shared = self.shared.clone();
            thread::Builder::new()
                .name("fileporter-dnssd".into())
                .spawn(move || Worker::new(shared).run(rx))
                .map_err(|error| error.to_string())?;
            self.commands = Some(tx);
        }
        Ok(self.commands.as_ref().expect("worker started"))
    }

    fn send(&mut self, command: Command) -> Result<(), String> {
        if self.ensure_running()?.send(command).is_err() {
            // The worker exited; start a fresh one on the next call.
            self.commands = None;
            return Err("dnssd_worker_stopped".into());
        }
        Ok(())
    }
}

impl Default for DnsSdDiscoveryAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscoveryAdapter for DnsSdDiscoveryAdapter {
    fn publish(&mut self, record: &DiscoveryRecord) -> Result<(), String> {
        self.send(Command::Publish(record.clone()))
    }
    fn withdraw(&mut self) -> Result<(), String> {
        self.send(Command::Withdraw)
    }
    fn browse(&mut self) -> Result<Vec<DiscoveryRecord>, String> {
        self.ensure_running()?;
        let mut shared = self.shared.lock().expect("dnssd shared state poisoned");
        if let Some(error) = shared.errors.pop() {
            shared.errors.clear();
            return Err(error.into());
        }
        Ok(std::mem::take(&mut shared.records))
    }
    fn departed(&mut self) -> Vec<String> {
        std::mem::take(
            &mut self
                .shared
                .lock()
                .expect("dnssd shared state poisoned")
                .departed,
        )
    }
}

/// One address lookup per host. Several instances can live on one host — two
/// simulators on one Mac, or this pad's own record beside a peer's — so the
/// lookup serves every instance resolved to it rather than the latest one.
struct HostLookup {
    sd_ref: DnsServiceRef,
    started: Instant,
    services: HashMap<Instance, (u16, Vec<u8>)>,
    addresses: HashSet<IpAddr>,
}

struct Worker {
    shared: Arc<Mutex<Shared>>,
    /// Boxed so its address is stable: every callback context points here.
    events: Box<EventQueue>,
    browse: Option<DnsServiceRef>,
    registration: Option<DnsServiceRef>,
    resolves: HashMap<Instance, DnsServiceRef>,
    /// Keyed by host and interface: a loopback answer must not stand in for
    /// the LAN address of the same host.
    lookups: HashMap<(String, u32), HostLookup>,
    known: HashSet<Instance>,
    last_resolve: Instant,
}

impl Worker {
    fn new(shared: Arc<Mutex<Shared>>) -> Self {
        Self {
            shared,
            events: Box::default(),
            browse: None,
            registration: None,
            resolves: HashMap::new(),
            lookups: HashMap::new(),
            known: HashSet::new(),
            last_resolve: Instant::now(),
        }
    }

    fn context(&mut self) -> *mut c_void {
        (&mut *self.events as *mut EventQueue).cast()
    }

    fn run(mut self, commands: mpsc::Receiver<Command>) {
        loop {
            loop {
                match commands.try_recv() {
                    Ok(Command::Publish(record)) => self.publish(&record),
                    Ok(Command::Withdraw) => self.withdraw(),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.teardown();
                        return;
                    }
                }
            }
            if self.browse.is_none() {
                self.start_browse();
            }
            if self.last_resolve.elapsed() >= RERESOLVE_EVERY {
                self.last_resolve = Instant::now();
                for instance in self.known.clone() {
                    self.resolve(instance);
                }
            }
            self.expire_lookups();
            self.pump(Duration::from_millis(250));
            self.handle_events();
        }
    }

    fn start_browse(&mut self) {
        let regtype = CString::new(regtype()).expect("regtype has no NUL");
        let mut sd_ref: DnsServiceRef = std::ptr::null_mut();
        let context = self.context();
        // SAFETY: every pointer is valid for the call; the context outlives the
        // ref because both are owned by this worker and released in teardown.
        let status = unsafe {
            DNSServiceBrowse(
                &mut sd_ref,
                0,
                0,
                regtype.as_ptr(),
                std::ptr::null(),
                on_browse,
                context,
            )
        };
        if status == 0 {
            self.browse = Some(sd_ref);
        } else {
            self.fail("mdns_browse_failed");
            // Local-network permission can be granted later; back off before
            // the loop tries again.
            thread::sleep(Duration::from_secs(2));
        }
    }

    fn publish(&mut self, record: &DiscoveryRecord) {
        self.withdraw();
        let txt = encode_txt(&[
            ("id", record.device_id.as_str()),
            ("name", record.device_name.as_str()),
            ("pin", record.certificate_fingerprint.as_str()),
            ("ver", &record.protocol_version.to_string()),
            ("caps", &record.capabilities.join(",")),
        ]);
        let (Ok(name), Ok(regtype)) = (
            CString::new(format!("fileporter-{}", record.device_id)),
            CString::new(regtype()),
        ) else {
            self.fail("mdns_publish_failed");
            return;
        };
        let mut sd_ref: DnsServiceRef = std::ptr::null_mut();
        let context = self.context();
        // SAFETY: as in start_browse; the TXT buffer is copied by the call.
        let status = unsafe {
            DNSServiceRegister(
                &mut sd_ref,
                0,
                0,
                name.as_ptr(),
                regtype.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                record.endpoint.port().to_be(),
                txt.len() as u16,
                txt.as_ptr().cast(),
                on_register,
                context,
            )
        };
        if status == 0 {
            self.registration = Some(sd_ref);
        } else {
            self.fail("mdns_publish_failed");
        }
    }

    fn withdraw(&mut self) {
        if let Some(sd_ref) = self.registration.take() {
            // SAFETY: the ref came from DNSServiceRegister and is released once.
            unsafe { DNSServiceRefDeallocate(sd_ref) };
        }
    }

    fn resolve(&mut self, instance: Instance) {
        if self.resolves.contains_key(&instance) {
            return;
        }
        let (Ok(name), Ok(regtype), Ok(domain)) = (
            CString::new(instance.name.clone()),
            CString::new(regtype()),
            CString::new(instance.domain.clone()),
        ) else {
            return;
        };
        let mut sd_ref: DnsServiceRef = std::ptr::null_mut();
        let context = self.context();
        // SAFETY: as in start_browse.
        let status = unsafe {
            DNSServiceResolve(
                &mut sd_ref,
                0,
                instance.interface,
                name.as_ptr(),
                regtype.as_ptr(),
                domain.as_ptr(),
                on_resolve,
                context,
            )
        };
        if status == 0 {
            self.resolves.insert(instance, sd_ref);
        }
    }

    fn look_up(&mut self, host: String, instance: Instance, port: u16, txt: Vec<u8>) {
        let key = (host.clone(), instance.interface);
        if let Some(existing) = self.lookups.get_mut(&key) {
            existing.started = Instant::now();
            existing.services.insert(instance.clone(), (port, txt));
            let records: Vec<DiscoveryRecord> = existing
                .addresses
                .iter()
                .filter_map(|address| {
                    let (port, txt) = existing.services.get(&instance)?;
                    record_from(txt, SocketAddr::new(*address, *port))
                })
                .collect();
            self.publish_records(records);
            return;
        }
        let Ok(hostname) = CString::new(host.clone()) else {
            return;
        };
        let mut sd_ref: DnsServiceRef = std::ptr::null_mut();
        let context = self.context();
        // SAFETY: as in start_browse.
        let status = unsafe {
            DNSServiceGetAddrInfo(
                &mut sd_ref,
                0,
                instance.interface,
                PROTOCOL_IPV4 | PROTOCOL_IPV6,
                hostname.as_ptr(),
                on_address,
                context,
            )
        };
        if status == 0 {
            self.lookups.insert(
                key,
                HostLookup {
                    sd_ref,
                    started: Instant::now(),
                    services: HashMap::from([(instance, (port, txt))]),
                    addresses: HashSet::new(),
                },
            );
        }
    }

    fn publish_records(&self, records: Vec<DiscoveryRecord>) {
        if records.is_empty() {
            return;
        }
        self.shared
            .lock()
            .expect("dnssd shared state poisoned")
            .records
            .extend(records);
    }

    fn expire_lookups(&mut self) {
        let expired: Vec<(String, u32)> = self
            .lookups
            .iter()
            .filter(|(_, lookup)| lookup.started.elapsed() >= ADDRESS_WINDOW)
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            if let Some(lookup) = self.lookups.remove(&key) {
                // SAFETY: removed from the map, so released exactly once.
                unsafe { DNSServiceRefDeallocate(lookup.sd_ref) };
            }
        }
    }

    /// Waits for any ref's socket to become readable and lets the responder
    /// run the callbacks for it.
    fn pump(&mut self, timeout: Duration) {
        let refs: Vec<DnsServiceRef> = self
            .browse
            .iter()
            .chain(self.registration.iter())
            .chain(self.resolves.values())
            .chain(self.lookups.values().map(|lookup| &lookup.sd_ref))
            .copied()
            .collect();
        if refs.is_empty() {
            thread::sleep(timeout);
            return;
        }
        let mut fds: Vec<PollFd> = refs
            .iter()
            // SAFETY: each ref is live until teardown or explicit removal.
            .map(|sd_ref| PollFd {
                fd: unsafe { DNSServiceRefSockFD(*sd_ref) },
                events: POLLIN,
                revents: 0,
            })
            .collect();
        // SAFETY: `fds` is a valid, correctly sized buffer.
        let ready = unsafe {
            poll(
                fds.as_mut_ptr(),
                fds.len() as u32,
                timeout.as_millis() as c_int,
            )
        };
        if ready <= 0 {
            return;
        }
        for (sd_ref, fd) in refs.iter().zip(fds.iter()) {
            if fd.revents != 0 {
                // SAFETY: the ref is live; callbacks only append to `events`.
                let status = unsafe { DNSServiceProcessResult(*sd_ref) };
                if status != 0 && Some(*sd_ref) == self.browse {
                    // SAFETY: the browse ref is dropped here and restarted.
                    unsafe { DNSServiceRefDeallocate(*sd_ref) };
                    self.browse = None;
                    self.fail("mdns_browse_failed");
                    return;
                }
            }
        }
    }

    fn handle_events(&mut self) {
        let events = std::mem::take(&mut self.events.0);
        for event in events {
            match event {
                Event::Found(instance) => {
                    if device_id_from_instance(&instance.name).is_some() {
                        self.known.insert(instance.clone());
                        self.resolve(instance);
                    }
                }
                Event::Lost(instance) => {
                    self.known.remove(&instance);
                    if let Some(sd_ref) = self.resolves.remove(&instance) {
                        // SAFETY: removed from the map, so released once.
                        unsafe { DNSServiceRefDeallocate(sd_ref) };
                    }
                    // A device on two interfaces is only gone once every
                    // interface has reported it gone.
                    let still_seen = self.known.iter().any(|known| known.name == instance.name);
                    if let (false, Some(device_id)) =
                        (still_seen, device_id_from_instance(&instance.name))
                    {
                        self.shared
                            .lock()
                            .expect("dnssd shared state poisoned")
                            .departed
                            .push(device_id);
                    }
                }
                Event::Resolved {
                    instance,
                    host,
                    port,
                    txt,
                } => {
                    if let Some(sd_ref) = self.resolves.remove(&instance) {
                        // SAFETY: one answer is enough; released once.
                        unsafe { DNSServiceRefDeallocate(sd_ref) };
                    }
                    self.look_up(host, instance, port, txt);
                }
                Event::Address {
                    host,
                    interface,
                    address,
                } => {
                    if !crate::engine::is_loopback_or_private(address)
                        || address.is_loopback()
                        || matches!(address, IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80)
                    {
                        continue;
                    }
                    let Some(lookup) = self.lookups.get_mut(&(host, interface)) else {
                        continue;
                    };
                    lookup.addresses.insert(address);
                    let records = lookup
                        .services
                        .values()
                        .filter_map(|(port, txt)| record_from(txt, SocketAddr::new(address, *port)))
                        .collect();
                    self.publish_records(records);
                }
                Event::Failed(code) => self.fail(code),
            }
        }
    }

    fn fail(&self, code: &'static str) {
        self.shared
            .lock()
            .expect("dnssd shared state poisoned")
            .errors
            .push(code);
    }

    fn teardown(&mut self) {
        let refs: Vec<DnsServiceRef> = self
            .browse
            .take()
            .into_iter()
            .chain(self.registration.take())
            .chain(self.resolves.drain().map(|(_, sd_ref)| sd_ref))
            .chain(self.lookups.drain().map(|(_, lookup)| lookup.sd_ref))
            .collect();
        for sd_ref in refs {
            // SAFETY: every ref was taken out of its owner above.
            unsafe { DNSServiceRefDeallocate(sd_ref) };
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.teardown();
    }
}

fn events_from(context: *mut c_void) -> &'static mut Vec<Event> {
    // SAFETY: every ref is created with the worker's boxed event queue as its
    // context and is only processed on the worker thread that owns it.
    unsafe { &mut (*context.cast::<EventQueue>()).0 }
}

fn string_from(pointer: *const c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the responder passes NUL-terminated strings valid for the call.
    Some(
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned(),
    )
}

extern "C" fn on_browse(
    _: DnsServiceRef,
    flags: DnsServiceFlags,
    interface: u32,
    error: DnsServiceError,
    name: *const c_char,
    _: *const c_char,
    domain: *const c_char,
    context: *mut c_void,
) {
    let events = events_from(context);
    if error != 0 {
        events.push(Event::Failed("mdns_browse_failed"));
        return;
    }
    let (Some(name), Some(domain)) = (string_from(name), string_from(domain)) else {
        return;
    };
    let instance = Instance {
        name,
        domain,
        interface,
    };
    events.push(if flags & FLAGS_ADD != 0 {
        Event::Found(instance)
    } else {
        Event::Lost(instance)
    });
}

extern "C" fn on_resolve(
    _: DnsServiceRef,
    _: DnsServiceFlags,
    interface: u32,
    error: DnsServiceError,
    fullname: *const c_char,
    host: *const c_char,
    port: u16,
    txt_len: u16,
    txt: *const u8,
    context: *mut c_void,
) {
    if error != 0 {
        return;
    }
    let (Some(fullname), Some(host)) = (string_from(fullname), string_from(host)) else {
        return;
    };
    let txt = if txt.is_null() {
        Vec::new()
    } else {
        // SAFETY: the responder guarantees `txt_len` readable bytes.
        unsafe { std::slice::from_raw_parts(txt, txt_len as usize) }.to_vec()
    };
    // The fullname is `<escaped instance>._fileporter._tcp.<domain>`.
    let Some((name, domain)) = split_fullname(&fullname) else {
        return;
    };
    events_from(context).push(Event::Resolved {
        instance: Instance {
            name,
            domain,
            interface,
        },
        host,
        port: u16::from_be(port),
        txt,
    });
}

extern "C" fn on_address(
    _: DnsServiceRef,
    flags: DnsServiceFlags,
    interface: u32,
    error: DnsServiceError,
    host: *const c_char,
    address: *const u8,
    _: u32,
    context: *mut c_void,
) {
    if error != 0 || address.is_null() || flags & FLAGS_ADD == 0 {
        return;
    }
    let Some(host) = string_from(host) else {
        return;
    };
    // SAFETY: a BSD sockaddr: byte 1 is the family, and the responder supplies
    // a full sockaddr_in or sockaddr_in6 for that family.
    let address = unsafe {
        match *address.add(1) {
            AF_INET => {
                let octets = std::slice::from_raw_parts(address.add(4), 4);
                IpAddr::V4(Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3]))
            }
            AF_INET6 => {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(std::slice::from_raw_parts(address.add(8), 16));
                IpAddr::V6(Ipv6Addr::from(octets))
            }
            _ => return,
        }
    };
    events_from(context).push(Event::Address {
        host,
        interface,
        address,
    });
}

extern "C" fn on_register(
    _: DnsServiceRef,
    _: DnsServiceFlags,
    error: DnsServiceError,
    _: *const c_char,
    _: *const c_char,
    _: *const c_char,
    context: *mut c_void,
) {
    if error != 0 {
        events_from(context).push(Event::Failed("mdns_publish_failed"));
    }
}

fn device_id_from_instance(name: &str) -> Option<String> {
    name.strip_prefix("fileporter-")
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// Splits a DNS-SD fullname into its unescaped instance label and domain.
fn split_fullname(fullname: &str) -> Option<(String, String)> {
    let marker = format!(".{}.", regtype());
    let index = fullname.find(&marker)?;
    let (escaped, rest) = fullname.split_at(index);
    let domain = rest[marker.len()..].to_owned();
    let mut name = String::new();
    let mut chars = escaped.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\\' {
            name.push(character);
            continue;
        }
        let digits: String = chars.clone().take(3).collect();
        if digits.len() == 3 && digits.chars().all(|digit| digit.is_ascii_digit()) {
            let value: u32 = digits.parse().ok()?;
            name.push(char::from_u32(value)?);
            chars.nth(2);
        } else if let Some(escaped) = chars.next() {
            name.push(escaped);
        }
    }
    Some((name, domain))
}

fn encode_txt(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (key, value) in pairs {
        let entry = format!("{key}={value}");
        let bytes = &entry.as_bytes()[..entry.len().min(255)];
        out.push(bytes.len() as u8);
        out.extend_from_slice(bytes);
    }
    out
}

fn decode_txt(txt: &[u8]) -> HashMap<String, String> {
    let mut values = HashMap::new();
    let mut rest = txt;
    while let Some((&length, tail)) = rest.split_first() {
        let length = (length as usize).min(tail.len());
        let (entry, next) = tail.split_at(length);
        if let Some((key, value)) = String::from_utf8_lossy(entry).split_once('=') {
            values.insert(key.to_ascii_lowercase(), value.to_owned());
        }
        rest = next;
    }
    values
}

fn record_from(txt: &[u8], endpoint: SocketAddr) -> Option<DiscoveryRecord> {
    if endpoint.port() == 0 {
        return None;
    }
    let values = decode_txt(txt);
    Some(DiscoveryRecord {
        device_id: values.get("id")?.clone(),
        device_name: values.get("name")?.clone(),
        endpoint,
        certificate_fingerprint: values.get("pin")?.clone(),
        protocol_version: values.get("ver")?.parse().ok()?,
        capabilities: values
            .get("caps")
            .map(|caps| {
                caps.split(',')
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txt_round_trips_the_desktop_record_fields() {
        let txt = encode_txt(&[
            ("id", "abc"),
            ("name", "Office Mac"),
            ("pin", "blake3:00ff"),
            ("ver", "1"),
            ("caps", "receive-v1,pairing-v1"),
        ]);
        let record = record_from(&txt, "192.168.1.9:48721".parse().unwrap()).unwrap();
        assert_eq!(record.device_id, "abc");
        assert_eq!(record.device_name, "Office Mac");
        assert_eq!(record.certificate_fingerprint, "blake3:00ff");
        assert_eq!(record.protocol_version, 1);
        assert_eq!(record.capabilities, vec!["receive-v1", "pairing-v1"]);
    }

    #[test]
    fn records_without_identity_or_port_are_ignored() {
        let txt = encode_txt(&[("name", "No id"), ("ver", "1")]);
        assert!(record_from(&txt, "192.168.1.9:1".parse().unwrap()).is_none());
        let full = encode_txt(&[("id", "a"), ("name", "b"), ("pin", "c"), ("ver", "1")]);
        assert!(record_from(&full, "192.168.1.9:0".parse().unwrap()).is_none());
    }

    #[test]
    fn truncated_txt_does_not_read_past_the_buffer() {
        assert!(decode_txt(&[40, b'i', b'd']).is_empty());
    }

    #[test]
    fn fullnames_unescape_to_the_published_instance() {
        let (name, _) = split_fullname("fileporter-a\\032b._fileporter._tcp.local.").unwrap();
        assert_eq!(name, "fileporter-a b");
        let (name, domain) = split_fullname("fileporter-abc\\.d._fileporter._tcp.local.").unwrap();
        assert_eq!(name, "fileporter-abc.d");
        assert_eq!(domain, "local.");
        assert_eq!(device_id_from_instance(&name).as_deref(), Some("abc.d"));
    }
}
