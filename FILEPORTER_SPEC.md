# Fileporter v1 — Product and Engineering Specification

Status: implementation source of truth
Target product: Fileporter ("file transporter")
Target platforms: Windows and macOS
Application framework: Tauri 2
Protocol version: 1

## 1. Product definition

Fileporter is a personal, direct local-network file transfer application. A user drops files or folders into Fileporter on one computer, or selects them with a native file picker, and Fileporter immediately sends the selected top-level items to every trusted Fileporter device that is online on the same local network. Each receiver saves the items automatically into its configured destination directory.

There is no account, cloud storage, relay, web service, or central coordinator. Each installed app discovers peers and makes an encrypted peer-to-peer connection directly to them.

Fileporter remains available after its main window is closed. It runs as a per-user login agent with a tray/menu-bar icon, accepts transfers in the signed-in user's session, and can use the user's file-system and clipboard APIs. It is deliberately not a privileged Windows service or macOS launch daemon: those contexts cannot reliably interact with the signed-in user's tray, clipboard, and selected folders.

### 1.1 Product promise

After one-time setup, Fileporter machines on the same private network find and remember each other automatically:

1. Drop a file or directory onto Fileporter.
2. Every trusted device that is online receives it automatically.
3. On a receiving device, click Copy or Cut/Move next to a received item and paste it into the final location using the operating system's file manager.

### 1.2 Definitions

- **Device**: one Fileporter installation and its stable cryptographic identity.
- **Trusted device**: a device whose identity was authenticated and mutually confirmed automatically or by matching-code confirmation, and has not been revoked.
- **Online device**: a trusted device currently discovered by mDNS or reachable at a saved manual private-network address.
- **Connected device**: a trusted, online device with a recently authenticated connection or successful health check.
- **Batch**: one user action containing one or more top-level files/directories.
- **Target**: one receiving device selected for a batch.
- **Item**: one top-level file or directory in a batch. A directory item can contain many manifest entries.

## 2. Firm v1 decisions

These decisions remove ambiguity and are requirements unless the user changes them before implementation.

- A drop sends to all trusted devices that are online at the moment the batch is created. The target set is snapshotted; devices appearing later do not unexpectedly receive the batch.
- The Home screen also permits deselecting individual online devices before a drop. “All online” is the default on every launch.
- If no device is online, dropped/selected items remain staged in the UI and are not silently queued to every offline device. The UI offers “Send when available” explicitly.
- Trusted receivers auto-accept. Unknown or revoked devices can never offer files.
- Authenticated trust-on-first-discovery is the default: both devices prove possession of their advertised identities and mutually confirm over the encrypted pairing channel. Users can require human matching-code confirmation in Settings for hostile or shared LANs.
- An interrupted in-progress transfer is durably resumable while the source paths still exist and have not changed.
- Existing destination content is never overwritten. Name collisions create a numbered copy.
- v1 transfers regular files and directories, including empty directories and hidden files. Symlinks, Windows junctions/reparse points, sockets, devices, FIFOs, ACLs, extended attributes, resource forks, sparse-file structure, and hard-link identity are not preserved. Unsupported entries are skipped and reported.
- File contents are streamed. A whole file or directory is never buffered in RAM or loaded into the webview.
- Transfer compression is off in v1. It adds CPU cost, provides little value for common already-compressed formats, and complicates resume.
- No analytics or telemetry is sent anywhere. The application makes no Internet request for its core operation.
- Update checking is out of scope for the first build. Signed manual installers/releases are sufficient; a signed Tauri updater can be added later without changing the transfer architecture.

## 3. Goals and non-goals

### 3.1 Goals

- A first-time user can configure a receive directory and have two machines discover and trust each other without reading documentation.
- A returning user can send files or complete directory trees with a single drag/drop.
- Closing the window does not stop discovery, receiving, sending, or resume.
- One sender can fan out the same batch to multiple peers concurrently.
- Cross-platform transfers preserve file bytes, directory structure, file names when representable, and modification time when the destination file system supports it.
- The app is secure against an untrusted person on the same network by default.
- A transfer survives connection loss, app restart, sleep/wake, and a peer changing between Wi-Fi and Ethernet.
- Every completed incoming top-level item can be revealed in the native file manager or placed on the native file clipboard.
- The codebase is maintainable, strongly typed, testable without two physical machines for most cases, and packageable on both operating systems.

### 3.2 Non-goals for v1

- Internet/WAN transfer, NAT traversal, relay service, accounts, or cloud history.
- Mobile platforms, Linux, browser clients, or a headless NAS build.
- Remote browsing of another device's files.
- Bi-directional folder synchronization or watch folders.
- Text/URL clipboard sync.
- Receiving from an unpaired device with a per-transfer prompt.
- Exact POSIX permissions, ownership, Finder tags, alternate data streams, xattrs, or resource-fork preservation.
- Delta encoding, content deduplication, compression, bandwidth schedules, or QoS.
- Administrator/system-wide service installation.

## 4. Supported environments

- Windows 10 22H2 and Windows 11, x64 for the first distributable. Keep Rust/platform code architecture-compatible with Windows ARM64, but ARM64 packaging is not a v1 blocker.
- macOS 12 or later, Apple Silicon and Intel. Produce native artifacts per architecture; a universal DMG is desirable but not required for the first local test.
- IPv4 and IPv6 LANs, including Wi-Fi and Ethernet.
- Same-link discovery through mDNS/Bonjour. A manual `host:port` pairing path covers networks that filter multicast and routed private VLANs.
- File sizes from zero bytes through at least 100 GiB and directory trees through at least 100,000 entries, subject to disk space and file-system limits.

## 5. User experience

### 5.1 First-run onboarding

The main window must open on first launch even when launched with `--background`.

1. **Welcome**
   - Explain in one sentence: “Send files directly to your other computers on this network.”
   - No account or sign-in.
2. **Name this device**
   - Pre-fill a friendly OS computer name.
   - Validate 1–48 Unicode scalar values; trim whitespace; names do not need to be unique.
3. **Choose receive folder**
   - Suggest `Downloads/Fileporter`.
   - Use a native directory picker.
   - Create the folder after confirmation if needed and test create/write/delete access with a uniquely named zero-byte probe.
   - Do not enable receiving until a writable folder exists.
4. **Allow local network access**
   - Explain why the OS may ask.
   - Start mDNS browsing/advertising while the app is foregrounded so macOS can show its Local Network prompt.
   - On Windows explain that the firewall prompt should be allowed on Private networks.
   - Show actionable recovery instructions when access is denied or discovery cannot start.
5. **Run in background**
   - “Launch Fileporter when I sign in” defaults on and can be disabled.
6. **Automatic discovery**
   - Finish onboarding directly; nearby Fileporter devices appear automatically.
   - Keep a private-address fallback for networks that block multicast.

Persist onboarding completion only after a valid receive directory is chosen. If onboarding is abandoned, background auto-start must not create an invisible, nonfunctional process.

### 5.2 Automatic trust and confirmation-required flow

1. Both devices advertise and discover each other on the private LAN.
2. A deterministic initiator opens the limited pairing TLS channel. The endpoint must present the exact identity and certificate fingerprint from discovery.
3. Both sides exchange fresh nonces and signed transcript proofs. No transfer is possible yet.
4. With automatic trust enabled on both sides, authenticated `PairConfirmed` frames are exchanged without UI and each side stores the other's pin.
5. If either side requires confirmation, both show the same six-digit authentication code and trust commits only after the stricter side confirms.
6. Both show as trusted and connected. Future sessions authenticate against the stored pins.

Provide Reject, Cancel, a 120-second expiry, rate limiting, and a Settings control to require matching-code confirmation. Automatic mode is authenticated TOFU, not proof of ownership; clearly advise using confirmation-required mode on shared or hostile networks.

### 5.3 Home screen

Use a quiet, native-adjacent layout rather than a web dashboard.

- Header: Fileporter wordmark, local device name, compact connection/status indicator, Settings button.
- Recipient strip: `All online (N)` selected by default plus a toggle/chip for each trusted online device. Offline trusted devices appear in a muted overflow panel, not in the default target set.
- Large drop surface: “Drop files or folders to send to N devices.”
- Two native-picker buttons: `Choose files` and `Choose folder`.
- When there are no selected online targets: “No devices online” plus Pair a device and Send when available actions. Do not pretend that a send occurred.
- Active transfer area: batch-level progress plus one target row per device.
- Recent activity: combined incoming and outgoing history, newest first.

Dropping onto any point in the main content should activate the drop surface. Use Tauri's native file drop event and never rely only on browser `DataTransfer`, because native paths are required and webview behavior differs by platform.

### 5.4 Send interaction

- A drop or picker selection immediately creates a batch when at least one target is selected. No confirmation dialog.
- A five-second non-blocking snackbar offers Cancel, while preparation starts immediately.
- Preparation scans metadata in the Rust backend and reports entry count/bytes as they become known.
- The UI reports `Preparing`, `Sending`, `Verifying`, `Complete`, `Partially complete`, `Paused`, or `Failed` using words as well as color.
- A batch progress bar represents aggregate acknowledged bytes across all targets. Each target row has its own progress and current rate.
- Cancel can cancel the entire batch or a single target. Cancellation is cooperative and leaves no finalized partial item at the destination.
- Retry is offered for retryable failure. It reuses the same manifest only after revalidating source file size and modification time.

### 5.5 Receive and history interaction

- Incoming batches save automatically. Notify when a batch completes or partially completes, naming the trusted sender and item count.
- A history row shows direction, peer, time, item count, size, result, and actual destination.
- Expanding an incoming row shows one row for each **top-level item originally selected by the sender**. Do not render every child in a large directory tree.
- Every completed incoming top-level item has:
  - `Copy`: put the existing path on the OS file clipboard.
  - Windows `Cut`: put the existing path on the OS file clipboard with the move drop effect.
  - macOS `Move`: put the file URL on the general pasteboard and show “Paste with ⌥⌘V in Finder to move.” Finder's public file-pasteboard model has no durable Windows-style cut marker; do not use undocumented Finder pasteboard types or delete a source after guessing that a paste occurred.
  - `Show`: reveal/select the item in Explorer or Finder.
- A batch-level action applies Copy/Cut/Move to all of its completed top-level items.
- Disable clipboard actions when the item has since been moved/deleted and show a clear explanation.

### 5.6 Settings

- Device name.
- Receive directory with Change and writeability status.
- Launch at login.
- Receiving enabled/paused.
- Desktop notifications.
- Trusted devices: name, online state, last seen, certificate fingerprint short form, auto-send eligibility, Rename locally, Forget/Revoke.
- Network: listening state, preferred port, discovered addresses, Add device by address, diagnostics. Advanced details stay collapsed.
- History retention: default 30 days; options 7, 30, 90, forever. Deleting history does not delete received files.
- About: version, protocol version, View logs, Quit Fileporter.

### 5.7 Tray/menu-bar behavior

- The tray icon exists whenever the process is running.
- Main-window close hides the window and keeps the process alive. It must not be described as quitting.
- A second app launch activates the existing instance and shows/focuses the main window.
- Tray left-click shows the window. Context menu:
  - Status: `N devices connected` (disabled informational row)
  - Show Fileporter
  - Receiving enabled (checked toggle)
  - Recent received item submenu when supported, otherwise omit
  - Settings
  - Quit Fileporter
- Quit gracefully pauses active work, flushes checkpoints, closes the listener, unregisters mDNS, and exits. It does not delete resumable staging data.
- Tray icon states: neutral/idle, active transfer, attention/error, receiving paused. On macOS use template artwork and a menu label/status rather than relying on color alone.

## 6. Visual and accessibility direction

- React + TypeScript + Vite in the Tauri webview.
- Use system fonts: `-apple-system`, `BlinkMacSystemFont`, `Segoe UI Variable`, `Segoe UI`, then sans-serif.
- Use a small tokenized CSS layer and accessible headless primitives only where they materially help. Avoid a heavy “admin dashboard” component library.
- Rounded but restrained surfaces, subtle borders, one accent color, platform-aware spacing, and clear progress animation.
- Support light, dark, and system appearance.
- Full keyboard operation, visible focus, semantic controls, ARIA labels, and announcements for status changes that do not spam screen readers.
- Minimum 4.5:1 contrast for normal text; do not communicate state by color alone.
- Respect reduced motion. Transfer bars can update without animated sweeping when reduced motion is enabled.
- Escape all peer names and file names as text. Never inject them as HTML.
- Target initial window size around 900×680 logical pixels, minimum 720×520, responsive down to the minimum.

## 7. System architecture

### 7.1 High-level shape

One per-user Tauri process owns both the background engine and the optional main window:

```text
React UI/WebView
    │ typed Tauri commands + events
    ▼
Rust application state
    ├── Lifecycle/tray/autostart/single-instance
    ├── Peer discovery and presence
    ├── Identity, pairing, and trust store
    ├── TLS connection/session manager
    ├── Transfer scheduler and streaming engine
    ├── Manifest/path policy and destination finalizer
    ├── SQLite repository and migrations
    ├── Native clipboard/reveal/notifications adapters
    └── Structured logs and diagnostics
           │
           └── direct TLS 1.3 TCP ── other Fileporter process
```

The webview is presentation only. Networking, filesystem traversal, hashing, persistence, path validation, native clipboard writes, and lifecycle decisions must remain in Rust. Hiding/destroying the webview cannot affect the engine.

### 7.2 Recommended repository layout

```text
/
├── FILEPORTER_SPEC.md
├── IMPLEMENTATION_GOAL.md
├── README.md
├── package.json
├── pnpm-lock.yaml
├── src/                         # React frontend
│   ├── app/
│   ├── components/
│   ├── features/{onboarding,send,history,devices,settings}/
│   ├── lib/bridge.ts
│   ├── types/generated.ts
│   └── styles/
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/main.json
│   ├── migrations/
│   ├── icons/
│   └── src/
│       ├── app.rs
│       ├── commands.rs
│       ├── state.rs
│       ├── domain/
│       ├── persistence/
│       ├── discovery/
│       ├── identity/
│       ├── protocol/
│       ├── transfer/
│       ├── filesystem/
│       ├── platform/{windows,macos}.rs
│       ├── tray.rs
│       ├── notifications.rs
│       └── diagnostics.rs
├── tests/                       # process/integration fixtures
├── scripts/                     # documented, cross-platform dev helpers
└── .github/workflows/
```

Modules may be split into a Rust workspace (`fileporter-core` plus the Tauri shell) if that makes headless integration testing substantially cleaner. Do not create a separate executable/service merely for architecture aesthetics.

### 7.3 Runtime managers

Create these long-lived Rust components behind `Arc` handles in managed Tauri state:

- `SettingsRepository`
- `TransferRepository`
- `IdentityManager`
- `DiscoveryManager`
- `PairingManager`
- `ConnectionManager`
- `TransferScheduler`
- `FilesystemService`
- `PlatformIntegration`
- `UiEventBus`
- `ShutdownCoordinator`

Managers communicate through typed Tokio channels and cancellation tokens. Avoid a global mutex around the entire app. Database work, directory scans, hashing, and native UI calls must not block the async network executor or Tauri main thread.

## 8. Process lifecycle

1. Acquire single-instance ownership.
2. Initialize structured logging and panic reporting to local logs.
3. Open SQLite, enable foreign keys and WAL mode, and run embedded migrations transactionally.
4. Load/generate the device identity.
5. Load settings and validate the receive directory without creating arbitrary paths from untrusted state.
6. Build tray and native event handlers.
7. If onboarding is complete, start listener, discovery, presence expiry, connection manager, and transfer-resume scheduler.
8. Create/show the webview only for normal launch, onboarding, an actionable error, notification activation, tray activation, or second-instance activation. `--background` otherwise starts hidden.
9. On sleep/network loss, mark peers offline and pause affected targets without failing them.
10. On wake/network change, restart advertisements/browse, re-resolve addresses, authenticate again, and resume.
11. On graceful quit, stop accepting new work, checkpoint, send best-effort pause/cancel state, shut down tasks with a bounded timeout, and exit.

Only an explicit Quit exits. Window close, Escape, or clicking the dock/taskbar close affordance hides.

## 9. Discovery and local-network boundary

### 9.1 Discovery

- Advertise and browse DNS-SD service `_fileporter._tcp.local.` using mDNS.
- Prefer a pure-Rust cross-platform implementation so Windows does not require Bonjour to be installed separately.
- Listen on a configurable preferred TCP port, default `42837`. If unavailable, bind an ephemeral port and advertise the actual bound port.
- TXT records contain only:
  - `v=1` (discovery schema)
  - `p=1` (maximum protocol version)
  - shortened device ID
  - friendly device name
  - operating-system family
  - listener port when not already provided by SRV
- Do not publish usernames, file paths, transfer activity, trust status, or secrets.
- Honor interface addition/removal and mDNS TTL. Mark a peer unavailable after its records expire or after 15 seconds without a refresh/health response.
- Deduplicate multiple interface addresses by stable device ID. Try viable resolved addresses with Happy-Eyeballs-like staggering rather than treating each address as a device.

### 9.2 Network scope

- Core transfer connections are permitted only to/from loopback in tests and private, link-local, or unique-local addresses in production: RFC1918 IPv4, IPv4 link-local, IPv6 link-local, and IPv6 ULA.
- Manual addresses must pass the same policy. A later advanced setting may broaden routed networks, but public Internet addresses are rejected in v1.
- Bind IPv4 and IPv6 where supported. Do not hard-code interface names such as `en0`.
- Never use a system proxy for peer connections.
- Discovery may not cross subnets. Manual address pairing is the documented fallback when devices are on routed private subnets or multicast is filtered.

### 9.3 OS permission handling

- macOS bundle metadata must contain a clear `NSLocalNetworkUsageDescription` and declare `_fileporter._tcp` in `NSBonjourServices`.
- Trigger first discovery in the visible onboarding window. If permission is denied, show instructions linking to System Settings > Privacy & Security > Local Network.
- Sign macOS builds with a stable code identity before realistic permission testing; changing unsigned/ad-hoc identities can make local-network permission behavior confusing.
- On Windows, bind as the current user and provide first-run guidance for allowing the signed executable on Private networks only. Do not run `netsh`, weaken the firewall, or require elevation.

## 10. Identity, encryption, and pairing

### 10.1 Device identity

- Generate a cryptographically random Ed25519 device identity on first run.
- Device ID is `base32(BLAKE3(identity_public_key))`; use the full value internally and a short prefix only for display/discovery.
- Generate a self-signed certificate bound to that identity and suitable for TLS 1.3 mutual authentication. The certificate is not a Web PKI identity.
- Store secret key material in Windows Credential Manager/macOS Keychain when the platform adapter supports it. If a narrowly scoped fallback file is required during development, place it only under app-local data, create it with user-only permissions before writing, never log it, and prominently mark the fallback in diagnostics.
- Store trusted peer public keys and certificate fingerprints in SQLite. Friendly names are presentation metadata and never authenticate a peer.

### 10.2 Transport security

- Direct TCP protected by TLS 1.3 using Rustls/Tokio-Rustls.
- Both peers present their device certificate.
- For a trusted connection, the custom verifier must require an exact pinned identity/certificate binding and prove possession of the private key. Never globally disable certificate verification.
- For an automatic-discovery or explicit, unexpired pairing session, an unknown self-signed device certificate may proceed only into the limited pairing state machine. No offer, manifest, path, or chunk frame is accepted until trust is committed.
- Negotiate ALPN `fileporter/1`.
- Refuse TLS versions below 1.3, invalid signatures, changed pins, replayed session nonces, and protocol downgrades.

### 10.3 Pairing transcript

The pairing state machine must include fresh 256-bit nonces from both sides and a transcript containing protocol version, both full public keys, both certificate fingerprints, both device IDs, both nonces, and initiator/responder roles. Both devices sign the canonical transcript.

The six-digit authentication code is derived from a domain-separated cryptographic hash of the canonical signed transcript, reduced without modulo bias. Display it as `123 456` when confirmation-required mode is active. Trust is written only after:

- both signatures verify,
- both sides send authenticated confirmation frames, automatically or after local human confirmation according to each device's setting,
- when confirmation is required, the codes match by human inspection,
- the session has not expired, and
- peer identity is not already pinned differently.

Commit trust transactionally. If either side rejects/times out, erase the pending session. Limit unknown pairing attempts per source address and globally, with a small bounded queue.

### 10.4 Revocation

Forgetting a device revokes its trust pin while retaining a durable deny record, closes active sessions, cancels or pauses targets for that peer, and prevents automatic repair. The remote device remains untrusted. A changed identity under the same friendly name must be surfaced as a different device, never silently accepted.

## 11. Wire protocol

### 11.1 Framing

The protocol runs only inside the authenticated TLS stream.

- Connection preface: ASCII `FILEPORTER` followed by a zero byte and big-endian protocol version `1`.
- Frame header: `kind: u8`, `payload_length: u32` big-endian.
- Control payloads: UTF-8 JSON with tagged message type, explicit field names, and a maximum payload of 1 MiB. Reject duplicate critical fields and unknown protocol-major versions. Forward-compatible unknown optional fields may be ignored.
- Data chunk payloads are binary to avoid base64 overhead:
  - batch UUID: 16 bytes
  - entry UUID: 16 bytes
  - offset: u64 big-endian
  - data length: u32 big-endian
  - BLAKE3 hash of this chunk: 32 bytes
  - raw data: at most 1 MiB
- Reject declared lengths above limits before allocation. Bound every queue/channel.

Use UUIDv7 for batches/entries in local storage and transmit their 16-byte representation. All times are RFC 3339 UTC in control frames and integer UTC timestamps in SQLite.

### 11.2 Required control messages

- `Hello`: protocol min/max, full device ID, display metadata, session nonce, capabilities.
- `Auth`: signed canonical session transcript.
- `PairRequest`, `PairProof`, `PairConfirmed`, `PairRejected`.
- `OfferStart`: batch ID, top-level item summaries, total logical bytes, total entry count, created time.
- `ManifestPage`: monotonically numbered page of bounded manifest entries plus final-page marker.
- `OfferAccept`: selected destination generation, resolved top-level names, available-space result, and resume checkpoints.
- `OfferReject`: stable reason code and safe human detail.
- `ChunkAck`: entry ID and highest contiguous durable byte offset.
- `EntryComplete`: entry ID, total size, sender's whole-file BLAKE3.
- `EntryVerified`: actual relative destination and verified hash.
- `BatchComplete`, `BatchReceipt`.
- `Pause`, `ResumeQuery`, `Cancel`.
- `Ping`, `Pong`.
- `Error`: stable code, retryable boolean, safe detail.

Define Rust enums and JSON fixtures for every message. Document field semantics and enforce state-valid messages; for example, a chunk before an accepted manifest is a protocol violation.

### 11.3 Session rules

- Maintain one authenticated control/data connection per peer when useful; reconnect on demand and keep idle connections for a short bounded interval.
- At most one actively streaming batch per target peer in v1. Different peers transfer concurrently, which provides fan-out without saturating one disk with arbitrary concurrency.
- Use a sliding window of no more than eight 1 MiB chunks. Receiver acknowledgements represent bytes flushed to the partial file and durably checkpointed at a bounded cadence.
- Duplicate data at or below the acknowledged contiguous offset is idempotently acknowledged, not appended twice.
- Heartbeats detect half-open connections without turning normal sleep into a hard failure.
- Every error crosses the wire as a stable machine code; remote free-form text is never shown as trusted markup.

## 12. Transfer engine

### 12.1 Batch creation and manifest

- The frontend passes selected native paths to one Rust command. The backend canonicalizes source roots, rejects duplicates/nested duplicate selections, and creates immutable top-level item IDs.
- Scan on a blocking worker pool with cancellation and progress events.
- Never follow symlinks or reparse points. Use `symlink_metadata`/platform equivalent at every entry.
- For each regular file capture source path (local only), relative component vector, logical size, modification time, and a change-detection fingerprint appropriate to the platform (at minimum size + high-resolution mtime; add file identity when available).
- Preserve empty directories as manifest entries.
- Source absolute paths never cross the network. A manifest transmits only safe relative components and metadata.
- Stream manifest pages into storage/protocol rather than retaining an unbounded JSON document.
- Revalidate a file immediately before opening and after streaming. If it changes, fail that entry with `source_changed`; do not claim a verified transfer.

### 12.2 Receiver preflight

- Confirm sender remains trusted and receiving is enabled.
- Validate entry count, total logical bytes, component lengths, tree depth, duplicate paths, and all path components before creating file content.
- Query free space. Reject cleanly if known required logical bytes plus a 256 MiB safety reserve exceed available space. Continue to handle later disk-full errors safely.
- Resolve all sanitized/collision-safe destination names before accepting. Return top-level name mapping to the sender.
- Create a per-batch staging root inside the selected receive directory, e.g. `.fileporter-staging/<batch-id>/`. Mark the root hidden on Windows. The app owns only this exact validated subtree.
- The staging tree must contain no followed symlink. Build it from freshly created directories and re-check before writes/finalization.

### 12.3 Filename and path policy

- Represent transmitted paths as a vector of Unicode components, never a slash-delimited string.
- Reject empty components, `.`, `..`, NUL, separators, absolute paths, drive prefixes, UNC prefixes, and a depth above 256.
- Limit each encoded component to 255 bytes where required and complete relative path to a conservative 32 KiB internal cap.
- On macOS preserve valid names as presented by the filesystem; do not perform lossy display normalization.
- On Windows replace forbidden characters/control characters, trailing spaces/dots, and reserved DOS device names with a safe form.
- If sanitization changes a name or causes a case-insensitive/canonical collision, append `~` plus the first eight hexadecimal characters of a BLAKE3 hash of the original component before the extension. Record a warning and actual mapping in history.
- Top-level destination collisions use native-looking numbered names (`Report (1).pdf`, `Photos (1)`). Reserve a name with create-new/no-replace semantics. If another process races, increment and retry.
- Never overwrite, merge into, or delete an existing destination tree.

### 12.4 Streaming and verification

- Send regular files in deterministic manifest order. Directories are created from manifest entries.
- Read and write fixed-size bounded buffers (default 1 MiB). Apply TCP and channel backpressure.
- Verify the BLAKE3 chunk hash before writing. Write only at the expected offset.
- Compute a whole-file BLAKE3 while receiving. On resume, re-read the existing partial prefix to rebuild the hasher before accepting new bytes.
- After `EntryComplete`, flush data, verify length and whole-file hash, apply modification time best-effort, then mark the entry verified.
- A mismatch fails the entry and keeps the partial only when an automatic retry can safely replace it; never finalize corrupt content.
- Zero-byte files still go through completion/verification.
- Checkpoint after each acknowledged window and on lifecycle pause. A checkpoint identifies verified entries and the current entry's contiguous offset.

### 12.5 Finalization

- Finalize only verified top-level items.
- Rename from staging into the configured destination with no-replace semantics on the same volume. A directory is exposed only after all supported descendants verify.
- If a batch contains independent top-level items, successfully verified items may finalize even if another top-level item fails; batch result becomes `partially_completed`.
- Store actual absolute received paths locally for history and clipboard operations.
- Remove the exact empty staging subtree after successful finalization. On cancellation remove only the validated batch staging subtree. On crash retain it for resume.
- Prune abandoned staging data after seven days only when no resumable transfer references it. Surface reclaimed byte count in logs.

### 12.6 Fan-out and scheduling

- A batch creates one durable target record per selected peer.
- Read each source independently per peer for v1. Do not implement an unbounded in-memory multicast buffer. OS file cache will help fan-out on a LAN.
- Transfer peers concurrently with default global maximum four active target streams and maximum one stream per peer.
- Queue additional targets FIFO with small fairness adjustments so one giant batch does not permanently starve later small batches.
- Pause/resume rather than fail on peer offline, socket reset, sleep/wake, or interface change. Exponential reconnect backoff starts near 1 second and caps near 30 seconds with jitter while the peer remains discoverable.
- Authentication failures, revoked peers, source changes, permission errors, and invalid manifests are terminal until explicit user action.

### 12.7 State machines

Batch states:

```text
staged → preparing → queued → active → verifying → completed
                    │          │           └────→ partially_completed
                    │          ├───────────────→ paused
                    └──────────────────────────→ failed
any nonterminal ───────────────────────────────→ canceled
```

Target states:

```text
queued → connecting → authenticating → offering → transferring → verifying → completed
   ▲          │              │             │             │
   └──────── paused/retryable_failure ◀────┴─────────────┘
terminal: failed | canceled
```

Persist transitions transactionally and validate them in one domain module. UI labels may be friendlier but may not invent state.

## 13. Persistence

Use an embedded SQLite database under Tauri's app-local data directory. Enable WAL, foreign keys, busy timeout, and numbered embedded migrations. Do not put the DB in the user-selected receive directory.

Minimum logical schema:

- `settings(key PRIMARY KEY, value_json, updated_at)`
- `trusted_peers(device_id PRIMARY KEY, public_key, certificate_fingerprint, remote_name, local_alias, paired_at, last_seen_at, auto_send, revoked_at)`
- `peer_addresses(device_id, address, port, interface_scope, source, last_seen_at)`
- `pairing_sessions(id PRIMARY KEY, peer_key, role, transcript_hash, expires_at, state)`; never persist secret ephemeral material beyond need
- `batches(id PRIMARY KEY, direction, state, created_at, completed_at, total_bytes, total_entries, warning_count)`
- `batch_targets(id PRIMARY KEY, batch_id, peer_device_id, state, acknowledged_bytes, error_code, retry_at)`
- `items(id PRIMARY KEY, batch_id, parent_item_id, kind, display_name, source_path_local, destination_path_local, size, mtime, state, warning_json)`
- `checkpoints(target_id, item_id, durable_offset, verified_hash, updated_at)`

It is acceptable to normalize manifest entries into a separate table or a compact local manifest file referenced by the DB when directory scale makes `items` too expensive. Whatever form is chosen must be versioned, bounded, crash-safe, and testable.

Paths and peer metadata are private local history. Logs should prefer IDs/counts over full paths. History retention deletes database history/checkpoints only after confirming no active/resumable work references them and never deletes finalized user files.

## 14. Tauri IPC contract

### 14.1 Commands

Expose a narrow, typed command surface. Commands that act on transfers/history should accept opaque IDs, not arbitrary destination paths.

- `get_app_snapshot()`
- `complete_onboarding(input)`
- `choose_receive_directory()`
- `update_settings(patch)`
- `choose_files()`
- `choose_directory()`
- `enqueue_paths({ paths, target_device_ids, queue_offline })`
- `cancel_batch(batch_id)`
- `cancel_target(target_id)`
- `retry_target(target_id)`
- `list_nearby_devices()`
- `request_pair(device_ref)`
- `confirm_pairing(session_id)`
- `reject_pairing(session_id)`
- `forget_device(device_id)`
- `set_item_clipboard({ item_id, operation })`
- `reveal_item(item_id)`
- `show_main_window()`
- `quit_app()`
- `export_diagnostics(destination_chosen_by_user)`

Picker commands return native paths to the backend flow without granting broad frontend filesystem read/write permission. `enqueue_paths` revalidates every path even when it came from a Tauri drag event.

### 14.2 Events

- `app://snapshot-changed`
- `peer://presence-changed`
- `pairing://changed`
- `transfer://batch-changed`
- `transfer://target-progress`
- `history://changed`
- `settings://changed`
- `app://attention-required`

On webview mount, subscribe first and then request a complete snapshot to avoid a race; use monotonic revision numbers to discard stale events. Throttle progress emission to roughly 5–10 updates per second per visible target.

Generate TypeScript DTO definitions from Rust at build/test time, or add a contract test that fails when mirrored types drift. All command errors use `{ code, message, retryable, field? }`; frontend does not parse error strings.

## 15. Platform integration

### 15.1 Shared Tauri behavior

- Tauri 2 stable releases and official plugins compatible with the selected Tauri minor.
- Single-instance plugin or equivalent native handling.
- Autostart plugin with a `--background` argument.
- Native dialog plugin for files/directories.
- Notification plugin for completed/failed incoming work, with permission requested in context.
- Rust-created tray and menu so it exists independently of the webview.
- Strict capabilities file: grant the main webview only the commands/plugins it needs.
- No remote webview content, navigation, eval, or blanket shell permission. Use a restrictive CSP and bundle all UI assets.

### 15.2 Windows

- Native clipboard module writes a `DROPFILES` structure in `CF_HDROP` with fully qualified, double-NUL-terminated UTF-16 paths.
- For Copy, register/write `Preferred DropEffect` as `DROPEFFECT_COPY`; for Cut, use `DROPEFFECT_MOVE`. Transfer ownership of allocated global memory correctly after `SetClipboardData` and retry briefly when the clipboard is busy.
- Reveal with `explorer.exe /select,<absolute-path>` using a direct process invocation with the path as an argument, never through a shell command string.
- Enable long-path-aware application metadata and test paths over 260 characters on a suitably configured machine.
- Package an NSIS per-user installer first. An MSI can be added if needed.
- The app runs unelevated. Code signing is required for a polished distributed build but absence of a certificate must not block a local development build.

### 15.3 macOS

- Use a small Rust AppKit bridge (`objc2` family or a narrowly scoped native module) to clear `NSPasteboard.general` and write one or more file `NSURL` objects with `writeObjects`.
- Copy and Move prepare the same public file URLs; the Move UI teaches the Finder-native `⌥⌘V` paste-to-move gesture. Do not use private Finder pasteboard types.
- Reveal with `/usr/bin/open -R <absolute-path>` through direct argument invocation, or use an equivalent public AppKit API.
- Use a status-item template icon. Normal Close hides the window; explicit Quit terminates.
- Bundle `NSLocalNetworkUsageDescription` and `NSBonjourServices` metadata.
- Build on macOS for macOS. Outside-App-Store distribution uses a signed/notarized DMG when credentials are available; ad-hoc/local signing is acceptable only for development.
- Do not enable macOS private Tauri APIs merely for visual effects.

## 16. Security requirements and threat model

Assume the LAN contains a curious or malicious untrusted device. Also assume a previously trusted peer may send hostile names, metadata, oversized messages, or executable file content.

Required controls:

- Mutual cryptographic identity, pinning, authenticated automatic first trust, optional SAS comparison, and revocation.
- TLS 1.3 encryption and transcript/session signatures.
- Strict network-scope policy and no Internet discovery/relay.
- State-machine validation for every wire message.
- Frame, manifest, path depth, entry count, allocation, concurrency, and pairing-rate bounds.
- No trust decisions based on IP address, hostname, mDNS TXT content, or friendly name.
- Path component validation, staging isolation, no symlink traversal, create-new/no-replace finalization.
- Disk-space preflight and safe disk-full handling.
- Files are inert bytes. Never execute, preview, index in HTML, or invoke a shell with received content.
- UI actions resolve database item IDs to known finalized paths. A compromised webview cannot ask Rust to copy/reveal/delete any arbitrary path.
- Tauri allowlist/capabilities are minimal; no remote origins.
- Secrets and full paths are redacted from normal logs. Diagnostic export requires an explicit save action and shows what is included.
- Dependency audit in CI (`cargo audit` or maintained equivalent and package-manager audit with an intentional policy).

Security limitations must be documented honestly:

- A user who confirms a mismatched pairing code defeats MITM protection.
- A trusted device can send unwanted or malicious file bytes because auto-receive is the requested behavior. OS malware scanning remains applicable; Fileporter does not claim to scan content.
- Anyone with access to the signed-in account and its credential store/file data can act as that device.

## 17. Reliability and error behavior

- All operations return stable error codes such as `network_permission_denied`, `peer_offline`, `peer_untrusted`, `authentication_failed`, `protocol_mismatch`, `source_missing`, `source_changed`, `destination_unwritable`, `insufficient_space`, `invalid_path`, `unsupported_entry`, `hash_mismatch`, `clipboard_busy`, and `internal`.
- Error copy must say what happened, what is safe, and what action is available. Never show raw Rust debug strings by default.
- File-level unsupported entries create warnings and may yield a partially completed batch; they do not crash the traversal.
- Network interruption pauses. Authentication or integrity failure fails loudly and does not auto-loop indefinitely.
- Persist before telling the sender an offset is durable.
- Reconcile SQLite state and staging directories on startup. Unknown staging directories are quarantined/logged, not blindly deleted.
- Device sleep, app restart, and address change are normal tested paths.
- Destination changes affect new incoming batches. Existing active/resumable batches remain bound to the destination generation/staging root they started with; the settings UI explains this.

## 18. Logging, privacy, and diagnostics

- Use `tracing` with daily or size-based rolling files under app-local logs, keeping roughly seven days and a bounded total size.
- Default log fields: timestamp, level, module, event code, batch/target/peer IDs (shortened), counts, byte totals, durations, and stable error code.
- Do not log file contents, private keys, auth transcripts, clipboard contents, or full absolute paths. Basenames should be omitted at info level and allowed only in a user-enabled diagnostic mode.
- Diagnostics view shows app/protocol/OS version, listener state, mDNS state, local interface summaries, trusted peer status, DB migration version, staging bytes, and recent error codes.
- Diagnostic export is a ZIP with redacted logs and a JSON summary. It does not include the DB, manifests, source/destination paths, identities, or file data.
- No telemetry endpoint, crash upload, or analytics SDK.

## 19. Performance budgets

These are engineering targets, not marketing guarantees:

- Idle CPU averages below 1% on a typical supported machine after discovery settles.
- Idle total memory target below 150 MiB including webview; hidden/background state may destroy or suspend the webview if doing so does not affect the engine.
- Backend transfer memory is bounded to tens of MiB and does not grow with file size or directory entry count.
- A trusted peer already visible should begin connection/preflight within about one second after batch preparation.
- On a healthy wired LAN with fast disks, the protocol should not be the dominant bottleneck. Avoid synchronous per-64-KiB round trips and base64 data.
- Progress remains responsive while hashing/transferring large files.

## 20. Technology choices

Use current stable, mutually compatible releases at implementation time and commit lockfiles. Do not pin this specification to patch versions that will immediately stale.

Backend baseline:

- Rust stable, Tokio, Tauri 2.
- `rustls`/`tokio-rustls` for TLS 1.3.
- `ed25519-dalek`, `rcgen`, `blake3`, `rand`/OS RNG for identity and hashes, subject to a coherent supported crypto provider.
- `mdns-sd` or an equivalently maintained pure-Rust DNS-SD crate.
- SQLite through `sqlx` or `rusqlite`; choose one and centralize access. Prefer compile-time migrations and a bundled SQLite build where needed.
- `serde`/`serde_json`, `uuid`, `time`, `thiserror`, `tracing`, `tokio-util` cancellation.
- Platform-gated `windows` crate and AppKit/Foundation bindings for clipboard/reveal details.

Frontend baseline:

- React, TypeScript strict mode, Vite, pnpm.
- Vitest + React Testing Library.
- `lucide-react` or a similarly lightweight, tree-shakable icon set.
- Plain tokenized CSS/CSS modules and accessible headless primitives as needed. A state library is optional; do not add one until React context/hooks become genuinely awkward.

Before adopting a crate/plugin, verify current maintenance, license, Tauri compatibility, and whether it introduces an external runtime. Keep a third-party notices/license process suitable for distribution.

## 21. Build, packaging, and developer experience

- Provide `README.md` with prerequisites, setup, development, tests, two-instance local smoke test, Windows build, macOS build, permission troubleshooting, architecture summary, and known limitations.
- Pin Node via `packageManager` and optionally `.node-version`; pin Rust stable via `rust-toolchain.toml` when reproducibility benefits.
- Core commands:
  - `pnpm install`
  - `pnpm tauri dev`
  - `pnpm lint`
  - `pnpm typecheck`
  - `pnpm test`
  - `cargo fmt --check --manifest-path src-tauri/Cargo.toml`
  - `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`
  - `cargo test --manifest-path src-tauri/Cargo.toml`
  - `pnpm tauri build`
- Add a test-only way to run two isolated cores/processes on loopback with separate temporary app-data/receive directories, unique ports, deterministic device names, and mDNS optionally disabled. This is never enabled in release builds.
- CI matrix runs frontend checks and Rust tests on Windows and macOS. Build unsigned artifacts on both; signed release jobs remain conditional on secrets.
- Do not commit signing keys, generated identities, databases, staging data, logs, `.env`, or build products.
- Produce app icons/tray icons in required platform sizes with a simple original Fileporter mark. Visual assets must work at 16–32 px and in monochrome macOS template form.

## 22. Test strategy

### 22.1 Rust unit tests

- State transition validity.
- Frame encode/decode and all size/overflow limits.
- Canonical pairing transcript and authentication-code test vectors.
- Address scope classification for IPv4/IPv6.
- Path component validation and Windows sanitization, including reserved names and case collisions.
- Collision naming and no-overwrite behavior.
- Manifest pagination and source-change detection.
- Checkpoint/resume math, duplicate chunks, zero-byte files, BLAKE3 mismatch.
- Error-code mapping and log redaction.

### 22.2 Rust integration tests

- Two authenticated in-process peers over loopback using temporary directories.
- Pair, reconnect from pinned identity, reject unknown/changed identity, revoke and reject.
- Single file, multiple top-level files, nested directory, empty directory, Unicode, hidden file, zero-byte file, and file larger than the chunk/window size.
- Fan-out to at least three receiving peer instances.
- Forced disconnect after deterministic chunk counts followed by correct resume and final hash.
- Sender restart and receiver restart from persisted checkpoints.
- Destination collision/race with proof existing bytes are unchanged.
- Malicious manifest path traversal, absolute paths, overlong frame, huge declared length, duplicate entries, symlink/reparse staging attack.
- Source mutation during transfer, receiver disk/write failure, cancellation, peer sleep/offline simulation.

### 22.3 Frontend tests

- Onboarding validation and permission-error recovery UI.
- Recipient selection defaults and snapshot semantics.
- Drop/picker behavior, no-target staging, cancel snackbar.
- Batch/target state rendering and accessible announcements.
- Pair code confirm/reject/timeout.
- History expansion and action enable/disable.
- Settings changes and destination-generation explanation.
- Light/dark/reduced-motion snapshots where useful; avoid brittle pixel-only tests.

### 22.4 Platform and manual acceptance

- Windows-to-Windows, macOS-to-macOS, Windows-to-macOS, macOS-to-Windows.
- Windows Explorer receives Copy and Cut clipboard payloads and pastes with correct copy/move semantics.
- Finder receives file URLs; normal paste copies and `⌥⌘V` moves.
- Closing both main windows still permits transfer.
- Launch-at-login starts hidden and tray is usable.
- Windows Private firewall flow and macOS Local Network allow/deny/re-enable flow.
- Sleep/wake, Wi-Fi disconnect/reconnect, Ethernet/Wi-Fi address change.
- Signed/notarized bundle identity retains macOS local-network permission across update where feasible.

## 23. Implementation milestones

Each milestone ends with tests and a runnable vertical slice. Do not postpone all integration until the end.

1. **Foundation**
   - Scaffold Tauri/React, strict lint/type checks, Rust modules, SQLite migrations, structured errors/logging, CI, basic window/tray/single-instance lifecycle.
2. **Onboarding and settings**
   - Device identity, receive-directory picker/write test, autostart, settings UI, background launch behavior.
3. **Discovery and trust**
   - TCP listener, mDNS, nearby UI, mutual TLS identity, pairing SAS, pin persistence, reconnect/revoke, manual address.
4. **Single-peer transfer vertical slice**
   - File picker/drop, one file manifest, chunk stream, verification, staging/finalize, progress/history.
5. **Complete filesystem semantics**
   - Multiple items, directories/empty dirs, path policy, name mapping/collisions, unsupported entries, large file bounds, cancellation, partial completion.
6. **Durability and fan-out**
   - Checkpoints/resume, restart/network transitions, scheduler, all-online target snapshot, multi-peer fan-out, reconciliation/pruning.
7. **Native finish**
   - Clipboard Copy/Cut/Move, reveal, notifications, polished tray, settings/devices/history UX, accessibility and themes.
8. **Hardening and distribution**
   - Threat tests, performance pass, docs, icons, Windows/macOS artifacts, signing hooks, complete manual acceptance matrix.

## 24. Acceptance criteria

Fileporter v1 is complete only when all of these are true:

1. Fresh installs on two supported machines can complete onboarding, discover and mutually trust one another without pairing UI on a normal private LAN, and reconnect without repeating first trust; confirmation-required mode also works with matching codes.
2. Dropping one or more files and complete directories sends them without another confirmation to every selected trusted online peer.
3. When both machines' main windows are closed but Fileporter remains in the tray/menu bar, an incoming transfer is accepted, saved, and notified.
4. Selected receive directories are honored. Existing files/directories are never overwritten or merged.
5. Received bytes match the sender's whole-file BLAKE3 for zero-byte, Unicode-name, multi-gigabyte, and nested-directory cases.
6. Disconnecting either peer mid-file and restarting one or both apps resumes from a durable checkpoint without duplicating/corrupting data.
7. A three-peer fan-out completes independently; one unavailable/full receiver does not fail successful targets.
8. Unknown, revoked, identity-changed, and pairing-rejected devices cannot send file metadata or content.
9. Traversal, symlink/reparse, oversized-frame, collision, and disk-full tests leave no writes outside the exact validated staging/destination paths and preserve pre-existing bytes.
10. Every finalized incoming top-level item can be revealed and copied through the native file clipboard; Windows Cut and Finder paste-to-move behavior pass manual tests.
11. Close-to-tray, explicit Quit, single-instance activation, launch-at-login, receive pause, sleep/wake, and network-change behaviors work as specified.
12. Frontend/Rust checks pass with no ignored critical test, no Clippy warnings under the repository command, and no committed secret/generated user data.
13. A Windows developer build launches successfully on the current Windows machine. macOS source compiles/tests in macOS CI or on a Mac runner; platform-only features have a documented manual test result before calling a public release complete.
14. README lets a new developer reproduce setup, tests, two-instance smoke test, and platform builds.

## 25. Definition of done and scope control

- The implementation satisfies this specification, acceptance criteria, and test plan; it is not merely scaffolded.
- No `TODO`, placeholder, mock, silent fallback, or disabled test remains on a required path.
- Any unavoidable platform limitation is documented in README and represented honestly in UI.
- Dependencies and all distributed assets have acceptable licenses and notices.
- The app is running locally on the current Windows machine in development or packaged form at handoff, with the exact launch command documented.
- A public release additionally requires real Windows signing and Apple Developer ID signing/notarization credentials, which are external prerequisites and must never be fabricated or committed.
- If implementation discovers a contradiction, prefer security, no-overwrite, and data integrity; record the decision in an Architecture Decision Record and update this specification before proceeding.

## 26. Explicitly deferred enhancements

- Optional offline queues to all trusted devices by default.
- Per-peer default recipient groups.
- QUIC/multi-stream transport after profiling proves TCP is limiting.
- File/folder context-menu share extension.
- Watch folders and bidirectional sync.
- Clipboard text/image sync.
- Signed auto-update feed.
- Linux/mobile.
- Routed/WAN connectivity, relay, or end-to-end account identity.
