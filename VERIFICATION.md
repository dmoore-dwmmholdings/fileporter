# Verification

Run these from the repository root.

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
./scripts/core-smoke.ps1
pnpm tauri build
```

Run the dependency checks used by CI with:

```powershell
cargo install cargo-audit --version 0.22.0 --locked
cargo audit --deny warnings --file crates/fileporter-protocol/Cargo.lock
cargo audit --deny warnings --file crates/fileporter-identity/Cargo.lock
cargo audit --deny warnings --file crates/fileporter-network/Cargo.lock
cargo audit --deny warnings --file crates/fileporter-transfer/Cargo.lock
cargo audit --deny warnings --file src-tauri/Cargo.lock
pnpm audit --audit-level=high
pnpm licenses:check
```

The Rust policy denies audit warnings across every locked Rust dependency. The
frontend policy audits all locked dependencies and fails high/critical advisories.
There are no current audit ignores; any temporary Rust exception must be an
advisory-specific, justified, expiring `audit.toml` entry with an upstream issue.

Third-party notices are generated from Cargo metadata for every checked Rust
lockfile and installed pnpm package metadata. Regenerate the committed,
metadata-only `THIRD_PARTY_MANIFEST.json` after a dependency change with
`pnpm licenses:generate`; `pnpm licenses:check` must be clean. The generator
fails closed on missing or non-allowlisted licenses and references dependency
source license files rather than reproducing license text. CI verifies drift and
uploads the resulting `third-party-notices` artifact for 14 days.

CI runs frontend lint/typecheck/test/build on Ubuntu, Rust format/strict Clippy/core
tests/desktop check on Windows and macOS, then builds and uploads unsigned Windows
MSI/NSIS and macOS DMG/app artifacts for 14 days. Build signed release artifacts
separately: Windows needs a code-signing certificate and WebView2 bootstrapper
policy; macOS needs an Apple Developer signing identity, hardened runtime, and
notarization credentials. Test macOS local-network permission on a signed build: the
app declares `_fileporter._tcp` and explains why it needs local-network access. Test
two physical/private-LAN peers, then test the manual private `host:port` route with
multicast disabled.

`pnpm tauri build` needs the platform packaging toolchain. On Windows, the GNU linker can fail while linking the desktop test DLL with `export ordinal too large`; use the MSVC Rust target/toolchain for release validation.

macOS builds target macOS 12.0 or later.
