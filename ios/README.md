# Fileporter for iPhone

A native SwiftUI app that speaks the same protocol as the desktop app. The
identity, pairing, TLS, and transfer engine are not reimplemented: the app links
the desktop's Rust core (`src-tauri`, built with the `mobile` feature) as a
static library and drives it through the small C interface in
`src-tauri/src/mobile.rs`. A pad linked from an iPhone is indistinguishable
from one linked from a computer.

## What is different on iPhone

- **Discovery** uses the system mDNSResponder (`src-tauri/src/discovery_dnssd.rs`)
  instead of the embedded `mdns-sd` responder. iOS refuses raw multicast sockets
  without a restricted entitlement; the records on the wire are identical.
- **Arrivals** land in `Documents/Received`, visible in the Files app under
  *On My iPhone › Fileporter*. Share, Quick Look, and Show in Files replace the
  desktop's Reveal, Copy, and Move.
- **Sending** copies picked photos, files, or folders into app storage first,
  because iOS only lends a picked file briefly and a held batch may wait hours
  for a dark pad. The copies are removed once the batch finishes or is cancelled.
- **Background**: iOS suspends a backgrounded app's sockets. A transport in
  flight gets the extra time iOS allows; then the core stops at a durable
  checkpoint and resumes when the app returns. Receiving only happens while
  Fileporter is open.
- There is no launch-at-login, tray, or receive-folder choice.

## Build and run

Requirements: Xcode 26 or later and Rust installed with rustup (the build adds
the `aarch64-apple-ios` and `aarch64-apple-ios-sim` targets on first use).

```sh
open ios/Fileporter.xcodeproj
```

Pick the *Fileporter* scheme and a simulator or your iPhone, then run. The
*Build Rust core* phase runs `ios/scripts/build-core.sh`, which compiles the
core in release mode for the selected platform. The first build takes a few
minutes; later builds are incremental. Signing uses team `P5GS55MLHL`; change
`DEVELOPMENT_TEAM` in the project to use another team.

From the command line:

```sh
# Unit tests (snapshot decoding, formatting, SVG arcs)
xcodebuild -project ios/Fileporter.xcodeproj -scheme Fileporter \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro' \
  -only-testing:FileporterTests test

# End to end: two simulators onboard, find and link each other over Bonjour,
# and exchange files through the real core.
xcodebuild -project ios/Fileporter.xcodeproj -scheme Fileporter \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro' \
  -destination 'platform=iOS Simulator,name=iPhone Air' \
  -maximum-concurrent-test-simulator-destinations 2 \
  -only-testing:FileporterUITests test
```

On first launch on a device, allow local-network access when asked; without
it the iPhone can neither see nor be seen by your other pads.

## Layout

| Path | Purpose |
| --- | --- |
| `Fileporter/Core` | The bridge to Rust, snapshot models, app model, and outbox |
| `Fileporter/Views` | Transport, Pads, Log, Config, onboarding, and the pad art |
| `Fileporter/Theme` | Colours, IBM Plex type, and shared controls from `src/styles/app.css` |
| `FileporterCore/include` | The C header and module map for the Rust library |
| `scripts/build-core.sh` | Builds the Rust static library for the current platform |
| `Fileporter/Assets.xcassets` | App icon and brand mark, from `branding/` (see `scripts/brand-assets.swift`) |

The IBM Plex fonts are bundled under the SIL Open Font License
(`Fileporter/Resources/Fonts/OFL-IBM-Plex.txt`).
