# ADR 0001: Device identity and certificate pinning

Fileporter creates one Ed25519 device identity per installation and binds its self-signed TLS certificate to that identity. Pairing uses signed, canonical transcripts and a human-verified SAS; trust records pin both the public key and certificate fingerprint. Names, addresses, and discovery records are presentation or reachability data, never authentication.

This prevents a LAN peer from becoming trusted merely by advertising a familiar name or endpoint. A changed identity or certificate is a new pairing decision.
