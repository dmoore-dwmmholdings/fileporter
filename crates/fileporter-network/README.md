# fileporter-network

Authenticated, direct TCP/TLS 1.3 session setup for Fileporter. Discovery, file transfer, disk
access, and persistence deliberately stay outside this crate.

## Security contract

- Each self-signed TLS certificate carries a critical Fileporter extension: the Ed25519 device
  public key and a signature over that key and the certificate TLS public key. The binding is
  verified before a peer description is returned.
- `TrustMode::Trusted` requires an exact device ID, Ed25519 public key, and full certificate
  fingerprint. A TLS handshake alone is not sufficient.
- `TrustMode::Pairing` accepts an otherwise unknown self-signed server certificate only after
  validating its Fileporter identity binding, and returns `SessionAuthorization::PairingOnly`.
  Callers must run the pairing/SAS flow before allowing offers or chunks.
- The server uses mandatory client certificates. This crate does not install an accept-all global
  verifier; its custom client verifier is scoped to a single `ClientConfig` and only validates the
  signed binding plus the requested trust mode.
- After TLS, both sides exchange the Fileporter v1 preface, `Hello`, and signed `Auth` messages
  under bounded timeouts. The resulting `AuthenticatedSession` proves possession of the pinned or
  pairing identity key, but does not authorize data frames itself.

## Integration

Create `LocalCertificate` from a persisted `DeviceIdentity` and retain its key/certificate in the
future credential-store adapter. Configure a listener with `server_config` and call
`accept_authenticated`; clients use `connect_authenticated`. Feed `PairingOnly` results into the
identity crate’s SAS flow, then persist an exact `TrustedPeerPin` transactionally.
