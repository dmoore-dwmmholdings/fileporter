# fileporter-identity

Pure Rust device-identity and pairing-cryptography domain for Fileporter v1. It does not access
the OS credential store, generate TLS certificates, open a socket, or make a trust decision.

## Integration

1. On first run, call `DeviceIdentity::generate`, then store `export_secret_bytes()` with a
   platform credential-store adapter. Recreate it with `from_secret_bytes` on future starts.
2. Persist `DevicePublicIdentity`, device ID, and the certificate fingerprint only after a pairing
   session reaches `PairingState::Confirmed` and both `PairingProof`s verify.
3. Build `PairingTranscript` from fresh 32-byte nonces, both roles, public keys, device IDs, and
   certificate fingerprints. The transcript itself verifies that each device ID is the BLAKE3/base32
   identifier of its public key.
4. Each device signs the exact canonical transcript bytes. Verify both proofs, then derive and
   visibly compare the six-digit SAS. Only call the local/remote confirmation transitions after
   human confirmation and before `expires_at`.

## Security invariants

- `DeviceIdentity` intentionally implements no `Debug`; secret export uses `Zeroizing<[u8; 32]>`.
- Transcript and proof encodings are binary and canonical, never JSON-map based. Participants and
  proofs are ordered lexicographically by public key while retaining their explicit roles.
- SAS reduction uses rejection sampling, not biased modulo reduction.
- Expiry/rejection are terminal. A new pairing attempt requires a new transcript and fresh nonces.
