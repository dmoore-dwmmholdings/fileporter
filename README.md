# Fileporter

Fileporter is a Windows/macOS desktop app for direct, trusted local-network file transfer. Devices discover each other through local DNS-SD/mDNS (`_fileporter._tcp.local.`), or can be paired using a validated private `host:port` endpoint when multicast is unavailable.

Every installation has an Ed25519 identity and an identity-bound self-signed TLS certificate. Pairing exchanges signed transcript proofs and a short authentication code (SAS); trust requires explicit confirmation on both devices and pins the identity and certificate. Discovery alone never grants trust or transfer permission.

Transfers use an authenticated pinned TLS connection, bounded protocol frames, staged receiving, durable acknowledgements/checkpoints, BLAKE3 verification, and no-overwrite finalization. The current release should still be validated with two physical peers before distribution.

## Quick start (Windows)

Prerequisites: Node.js with Corepack/pnpm, Rust stable, and the Windows WebView2 runtime.

```powershell
pnpm install
pnpm tauri dev
```

The first launch presents onboarding. Choose a writable receive folder; Fileporter probes it before saving the setting.

## Development checks

```powershell
pnpm typecheck
pnpm lint
pnpm test
cargo check --manifest-path src-tauri/Cargo.toml --no-default-features
pnpm tauri build
```

For the full command matrix, including strict Rust checks and audits, see [VERIFICATION.md](VERIFICATION.md).

## Local two-peer smoke

The repository has an isolated loopback smoke suite. It uses temporary state and
receive directories, deterministic peer identities, and no release-only switches;
it does not write to your normal Fileporter data.

```powershell
./scripts/core-smoke.ps1
```

It covers bilateral pairing confirmation, trusted discovery behavior, multi-entry
scheduling, interrupted-transfer resume, offline queue wake-up, and fan-out with a
failed sibling target. For real acceptance, use two physical private-LAN machines:
pair them, transfer files and directories, close both main windows to confirm tray
receiving, then repeat through the manual private `host:port` route with multicast
disabled. Those physical tests are not replaced by loopback coverage.

## Build and local-network troubleshooting

Windows development and unsigned package build:

```powershell
pnpm tauri dev
pnpm tauri build
```

Use the MSVC Rust toolchain for Windows packaging. A GNU desktop-test link can fail
with `export ordinal too large`; that is a toolchain/linker issue, not an app test
failure. Install WebView2 and allow the Windows firewall prompt on **Private**
networks. The resulting MSI/NSIS packages are development artifacts until signed.

On macOS 12 or later, install Xcode Command Line Tools, Node/pnpm, and Rust stable,
then run:

```bash
pnpm install --frozen-lockfile
pnpm tauri build
```

Allow Fileporter's Local Network prompt so it can browse and advertise
`_fileporter._tcp`; if it was denied, re-enable the app in macOS Privacy & Security
Local Network settings and relaunch. Unsigned macOS bundles are CI/development
artifacts. Distribution additionally needs an Apple Developer signing identity,
hardened runtime, and notarization.

## Architecture and data flow

The React/TypeScript webview is a typed presentation layer. It sends small Tauri
command DTOs and receives monotonic snapshot events. Rust owns every sensitive
operation: identity/key storage, TLS/pairing, mDNS and manual endpoint policy,
filesystem traversal, BLAKE3 hashing, SQLite persistence/checkpoints, scheduling,
native clipboard/reveal, notifications, and tray lifecycle.

```text
React webview -> typed Tauri commands/events -> Rust AppState
  -> identity + pinned TLS / discovery -> trusted LAN peer
  -> scheduler + streaming receiver -> staging + SQLite checkpoint
  -> verified no-overwrite finalization -> native reveal/copy/cut-or-move
```

Closing the window hides it; the Rust-owned listener, scheduler, and tray remain
alive until an explicit Quit. The webview never supplies arbitrary filesystem paths
to native completed-output actions: it supplies durable item or batch IDs only.

## Dependency-audit policy

CI installs pinned `cargo-audit` 0.22.0 and audits every Rust lockfile, denying
warnings. It also runs `pnpm audit --audit-level=high` against the frozen frontend
lockfile: high and critical advisories fail the build; lower severities are reported
but do not fail this gate. There are currently no audit ignores. A future Rust
advisory exception must be narrowly scoped in `audit.toml`, include the advisory ID,
upstream issue, justification, and expiry, and be removed when a fixed dependency is
available; do not weaken the severity policy globally.

## Third-party notices

`THIRD_PARTY_MANIFEST.json` is a checked-in, deterministic inventory of the exact
Rust lockfile and pnpm dependency metadata. It records each package's declared
license and upstream/source reference; it deliberately does not copy or invent
license text. The corresponding license text remains in the dependency source
file identified by the manifest (or upstream repository).

```powershell
pnpm licenses:generate
pnpm licenses:check
```

The generator fails closed for absent or non-allowlisted license expressions.
Allowed atoms are permissive licenses used by the current locked graphs; review a
new expression and its source license before extending that explicit policy. CI
regenerates in check mode and uploads the verified manifest as `third-party-notices`.

## Known limits and handoff

- Source and automated loopback coverage are not physical two-device LAN evidence.
  Still validate mDNS, the manual endpoint fallback, SAS comparison, firewall
  behavior, sleep/network changes, and tray receiving on real devices.
- macOS Local Network prompt/recovery and signed installer acceptance require macOS
  hardware. Windows/macOS signing and macOS notarization require external
  credentials and are intentionally not present in this repository.
- Native Explorer/Finder clipboard behavior and visual QA need manual desktop
  acceptance. The implementation uses only durable IDs for these actions.

The detailed evidence and remaining manual matrix are in [VERIFICATION.md](VERIFICATION.md);
the current evidence boundary is tracked in [PROJECT_STATE.md](PROJECT_STATE.md), and
design decisions are in [docs/adr](docs/adr).
