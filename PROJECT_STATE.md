# Fileporter project state ledger

Last reconciled: 2026-08-31
Authority: `FILEPORTER_SPEC.md` is the product/engineering source of truth; `IMPLEMENTATION_GOAL.md` is the execution contract. This ledger records inspected source and completed command output, not intent, mock UI copy, or physical-device acceptance.

## Objective and current boundary

Fileporter is a Windows/macOS Tauri application for direct transfer between explicitly paired, trusted private-LAN devices. The native implementation now covers identity-bound TLS, bilateral SAS pairing, trusted discovery/manual endpoint reachability, durable staged receive/resume/no-overwrite finalization, multi-entry/fan-out/offline scheduling, tray/background lifecycle, native completed-output reveal/copy/cut-or-move, and desktop snapshot-event delivery.

The repository has source and automated loopback coverage for these paths. It is **not** yet demonstrated as a signed, packaged application on two physical devices. Do not treat loopback tests, mocked frontend tests, or code inspection as browser visual QA, physical-LAN/macOS permission acceptance, or package-signing acceptance.

## Reconciled implementation inventory

| Slice | Current evidence | Current boundary |
| --- | --- | --- |
| Persistence, durability, history, and receiver | SQLite migrations `0001` through `0011`, `persistence.rs`, `state.rs`, `engine.rs`, and `fileporter-transfer` | SQLite persists public/application metadata: settings, trusted peers, pairing state, notification ledger, batches/targets/items/checkpoints and offline retry state. v10 migrates legacy v9 private identity/TLS blobs only after identity/certificate binding validation and successful platform-store writes, then transactionally deletes both legacy rows and records a migration marker. v11 persists the 0/7/30/90-day history-retention policy (30 default); pruning runs only after successful settings save and deletes terminal metadata, never finalized received files. History retains one batch row with nested incoming top-level durable item IDs/name/kind/size/state/availability for authorized Copy/Move/Reveal. Receive staging acknowledges only after durable write/checkpoint persistence, reconciles valid prefixes/mtime for resume, and does not advance acknowledged/checkpointed offset on injected write or fsync failure. |
| Identity, TLS, and pairing | `fileporter-identity`, `fileporter-network`, `identity.rs`, `secret_store.rs`, pairing listener tests | Ed25519 identity, certificate binding, signed role-aware transcript/challenge proofs, deterministic six-digit SAS, 120-second pending state, rate/size/time/replay bounds, and authenticated `PairConfirmed`/`PairRejected` exchange are implemented. Neither peer persists trust until both explicit confirmations have been exchanged and validated. Pairing mode does not grant transfer authorization. Windows Credential Manager/macOS Keychain are reached through maintained `keyring`, under stable app/device scope; secret bytes are not logged or serialized. In-memory-store tests cover first run/reload, v9 import, binding validation, failure retry and redaction. Physical SAS/device acceptance and independent security audit remain unproven. |
| Listener, discovery, and reachability | `engine.rs`, `listener_lifecycle.rs`, `discovery.rs`, `state.rs`, `commands.rs` | Onboarding and receiving preference reconcile listener start/stop/restart; discovery advertises/browses `_fileporter._tcp.local.`. Trusted presence is accepted only for a matching non-revoked trusted id/pin. Bounded, expiring nearby untrusted candidates retain validated private endpoint, advertised certificate fingerprint, version, and capabilities separately; competing fresh fingerprints for one id are rejected, and discovery never auto-trusts. `start_pairing_discovered` accepts only the current candidate id, resolves its stored endpoint, and enters the existing signed bilateral pairing flow; manual endpoints remain restricted to loopback/private/ULA ranges. Snapshots expose safe nearby candidates and actual trusted-online endpoints. Trusted-device aliases are persisted through validated ID-authorized rename. Discovery mock and listener/pairing loopback tests pass. Real interface churn, multicast-disabled networks, macOS local-network prompt/recovery, and physical-LAN acceptance remain unproven. |
| Scheduler, fan-out, event reactor, lifecycle, logging, and diagnostics | `state.rs`, `state_events.rs`, `app.rs`, `lifecycle_monitor.rs`, `logging.rs`, `discovery.rs`, `persistence.rs` | Durable outgoing queueing supports offline/waiting, cancellation/retry, recipient revocation checks, fan-out behavior, multi-entry scheduling and receiver resume coverage. `StateEvents` coalesces progress while delivering every terminal transition after durable persistence; desktop setup runs one owned reactor that emits snapshot refreshes without a later frontend command and drains on shutdown. Suspend is idempotent and pauses listener/discovery/scheduling after the durable checkpoint path; resume or network reconciliation rebinds/re-advertises and wakes waiting work without duplicate workers. A testable 30-second monotonic wake-gap detector invokes the suspend seam then reconnect reconciliation after a significant gap; the portable fallback is not a native interface-change subscription. Startup removes only Fileporter-owned rolling logs older than about seven days and caps retained logs at 32 MiB. Diagnostics expose truthful listener/bound state, mDNS lifecycle state, safe bound-interface summary, trusted-online endpoints, DB migration version, owned staging bytes, and a bounded/coalesced stable error-code list without paths or secrets. Crash/power-loss acceptance beyond injected durability faults remains unproven. |
| Notifications, autostart, tray, and native actions | `desktop_notifications.rs`, `commands.rs`, `desktop_actions.rs`, `tray.rs`, `app.rs` | Notification preference is checked before ledger claim/send; delivery is once-only, restart-safe and redacted. Onboarding/settings apply launch-at-login through an adapter and roll back settings on adapter failure. Tray header status and receiving check state refresh from the snapshot-event reactor with the actual trusted-online count. Its Settings action focuses the window and emits the app navigation event for Settings. The Recent received submenu is dynamically rebuilt from up to five safe, available completed incoming top-level items, ordered from persisted history; action IDs contain only durable item IDs and re-authorize/canonicalize through the existing completed-output resolver before reveal. It is disabled with a neutral empty item when no outputs are available. Reveal/Copy/Move resolve only completed incoming durable IDs to canonical existing paths. Windows Move publishes `CF_HDROP` plus `Preferred DropEffect=MOVE`; macOS prepares public Finder file URLs for the standard Option-Command-V move workflow without deleting a source. Adapter/model tests cover authorization, missing paths, batch behavior, Windows move payload, macOS argument safety, recent ordering/truncation/sanitization, and empty state. Native desktop interaction acceptance remains manual. |
| Frontend/IPC/settings | `src/App.tsx`, `src/components`, bridge/view-models, Rust commands | Onboarding provides network setup plus pair-or-skip flow. Settings include preferred listen address/port and persisted history retention; device pairing/SAS, discovered-candidate pairing, trusted rename, recipients, native picker/drop, queue/offline/cancel/retry, nested history-item and completed-output Copy/Move/Reveal actions are wired to native commands and snapshot events. Every finalized incoming top-level history item includes a safe destination label derived from its persisted output. Diagnostics expose actual listener/bound endpoint, mDNS state, safe trusted-online endpoint/interface/error summaries, DB migration version, staging-byte total, and app/protocol/log data. `view_logs` and `export_logs` use only app-owned canonical logs plus a native destination picker, never a webview-supplied path. Frontend test suite is recorded as 35 passing tests. It remains mocked bridge coverage: no attached browser surface visual QA has been performed. |
| Release metadata, icons, documentation, runtime, and CI | `src-tauri/tauri.conf.json`, `src-tauri/icons`, `src-tauri/Info.plist`, `README.md`, `VERIFICATION.md`, `docs/adr`, `.github/workflows/ci.yml`, license manifest/scripts | Product identifier/version/bundle fields, Windows MSI/NSIS metadata, macOS 12.0 minimum, local-network/Bonjour declaration for `_fileporter._tcp`, source SVG plus generated Tauri icon outputs, expanded README/verification guide, license manifest, and three ADRs are present. README documents the header status and license manifest workflow. The Windows MSVC MSI and NSIS packaging paths completed successfully. The runtime-spawner panic was fixed and a bounded Windows first-run smoke completed healthy. CI applies strict Rust clippy `-D warnings`, dependency auditing of all Rust lockfiles and high/critical frontend advisories, license-manifest drift checking, frontend lint/typecheck/test/build, Windows/macOS Rust format/core-test/desktop-check, and an unsigned Windows MSI/NSIS plus macOS DMG/app artifact job retained for 14 days. These CI artifacts are development evidence only; signing/notarization and installer acceptance remain external/manual. |

## Terminal validation evidence

| Date | Command/result | Scope and limit |
| --- | --- | --- |
| 2026-08-31 | `pnpm test` - passed, 35 frontend tests | Mocked frontend/UI/bridge coverage; not browser visual QA or native Tauri acceptance. |
| 2026-08-31 | `cargo test --manifest-path crates/fileporter-identity/Cargo.toml` - 8 passed | Identity/transcript/SAS primitives. |
| 2026-08-31 | `cargo test --manifest-path crates/fileporter-network/Cargo.toml` - 5 passed | TLS binding/pin and pairing authorization helpers. |
| 2026-08-31 | `cargo test --manifest-path crates/fileporter-protocol/Cargo.toml` - 8 passed | Framing, bounds, and transition helpers. |
| 2026-08-31 | `cargo test --manifest-path crates/fileporter-transfer/Cargo.toml` - 15 passed | Staging, no-overwrite, resume and injected write/fsync durability faults. |
| 2026-08-31 | `cargo test --manifest-path src-tauri/Cargo.toml --lib --no-default-features` - 78 passed | Core application state, OS-secret migration, pairing listener, discovery mock, scheduler, receiver, events, lifecycle, logging/diagnostics, notification, and native Copy/Move/Reveal adapter tests. |
| 2026-08-31 | Strict clippy: all four core crates and `src-tauri`, `--all-targets --all-features -- -D warnings`; plus Tauri no-default all-target clippy/check | Passed with zero warnings; CI applies Rust clippy with `-D warnings`. |
| 2026-08-31 | MSVC Windows Tauri bundle paths: MSI and NSIS | Both completed successfully. The first-run smoke after the runtime-spawner panic fix remained bounded and healthy. |
| 2026-08-31 | `cargo check --manifest-path src-tauri/Cargo.toml --features desktop` | Passed. |
| 2026-08-31 | `./scripts/core-smoke.ps1` | Passed focused pairing bilateral-confirmation, discovery adapter, multi-entry scheduler, resume, offline and fan-out scenarios. |
| 2026-08-31 | `.github/workflows/ci.yml` unsigned bundle job | Windows MSI/NSIS and macOS DMG/app CI artifacts build unsigned after frontend/Rust gates and are retained 14 days; this is not signing/notarization or installer acceptance. |

## Remaining unproven evidence and release blockers

1. No attached browser surface visual QA has been run; frontend tests do not verify rendered layout, native dialogs, tray/menu interaction, or snapshot-event UX.
2. No physical two-device private-LAN transfer/pairing acceptance exists, including real mDNS, multicast-disabled manual endpoint fallback, firewall/interface churn, or SAS comparison between devices.
3. No macOS Local Network/Bonjour permission prompt and recovery acceptance exists on macOS 12.0+.
4. Windows MSI/NSIS build paths and a bounded first-run smoke are complete, but Windows signing and installer acceptance, macOS packaging/signing/notarization, and cross-platform installer acceptance are not recorded.
5. An independent security review remains unproven despite the credential-store migration.
6. Crash/power-loss/restart behavior beyond the covered injected durability/resume tests needs system-level acceptance.

## Ordered roadmap

| ID | Status | Next deliverable |
| --- | --- | --- |
| FP-001 | partial | Run attached-browser visual QA across onboarding, pairing, settings, transfers/history, native actions, tray and proactive snapshot refresh. |
| FP-002 | partial | Run physical two-device private-LAN acceptance: mDNS, bilateral SAS confirmation, transfer/resume/fan-out, and manual endpoint fallback with multicast disabled. |
| FP-003 | partial | Validate macOS 12.0+ Local Network/Bonjour declarations and prompt/recovery behavior on signed hardware. |
| FP-004 | partial | Complete Windows signing/installer acceptance and macOS package/signing/notarization acceptance; capture artifact evidence. |
| FP-005 | partial | Complete an independent security review of the credential-store, pairing, pinning and transfer boundaries. |
| FP-006 | partial | Perform crash/power-loss and longer-running interface-churn/restart acceptance. |

## Verification commands

Run these from the repository root. Preserve these commands when updating this ledger.

```powershell
pnpm install --frozen-lockfile
pnpm typecheck
pnpm lint
pnpm test
pnpm build
cargo test --manifest-path crates/fileporter-identity/Cargo.toml
cargo test --manifest-path crates/fileporter-network/Cargo.toml
cargo test --manifest-path crates/fileporter-protocol/Cargo.toml
cargo test --manifest-path crates/fileporter-transfer/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml --no-default-features
cargo test --manifest-path src-tauri/Cargo.toml --lib --no-default-features
cargo check --manifest-path src-tauri/Cargo.toml --features desktop
./scripts/core-smoke.ps1
pnpm tauri build
```

Dependency checks used by CI are documented in `VERIFICATION.md`: `cargo-audit`
0.22.0 with `--deny warnings` against every Rust lockfile, plus
`pnpm audit --audit-level=high`.

For strict warning validation:

```powershell
cargo clippy --manifest-path crates/fileporter-identity/Cargo.toml --all-targets --all-features -- -D warnings
cargo clippy --manifest-path crates/fileporter-network/Cargo.toml --all-targets --all-features -- -D warnings
cargo clippy --manifest-path crates/fileporter-protocol/Cargo.toml --all-targets --all-features -- -D warnings
cargo clippy --manifest-path crates/fileporter-transfer/Cargo.toml --all-targets --all-features -- -D warnings
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --no-default-features -- -D warnings
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
```

## Compaction pickup

1. Read this ledger, `FILEPORTER_SPEC.md`, `IMPLEMENTATION_GOAL.md`, `README.md`, and `VERIFICATION.md`; inspect current source before making a status claim. This workspace has no usable Git metadata for history-based claims.
2. Preserve the distinction between completed terminal evidence, source/mock coverage, browser visual QA, physical-LAN acceptance, and signed-package acceptance. Never convert one category into another.
3. Keep the exact verification commands above and update their results/counts only after a command reaches a final terminal result.
4. Prioritize the remaining evidence in roadmap order: visual QA, physical two-device/private-LAN and macOS permission testing, package/signing acceptance, then security and crash/restart acceptance.
5. Update this ledger after each slice with exact files, commands/results, counts, and limits; do not reintroduce superseded status assertions.
