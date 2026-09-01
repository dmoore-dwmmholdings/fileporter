# fileporter-protocol

Pure Rust implementation of the Fileporter v1 wire format. This crate deliberately has no
socket, TLS, Tauri, filesystem, or persistence dependency.

## Integration API

- Send `PROTOCOL_PREFACE` once inside an already-authenticated TLS stream, then call
  `encode_control` or `encode_chunk` and write the returned bytes.
- Buffer incoming bytes at the transport layer and call `decode_frame` only with a complete
  frame (`FRAME_HEADER_LEN + declared payload length`). `frame_len` reports the required size
  once a header is available.
- Feed each decoded `Frame` through `SessionValidator::validate`. It rejects control/data
  messages that are invalid for the current protocol phase, including chunks before an accepted
  offer. The transfer engine must additionally validate batch/entry IDs and durable offsets.

Limits are part of the public contract: control payloads are at most 1 MiB and chunk data is at
most 1 MiB. `decode_chunk` verifies the BLAKE3 hash before returning data.
