# fileporter-transfer

Pure transfer-domain crate for the future scheduler/protocol layers. Call
`build_source_manifest` on a blocking worker with a `CancellationToken`, persist
the returned local-only source paths privately, and transmit only `components`
and metadata from its entries. Before receiver staging, call
`validate_receiver_components` and `sanitize_windows_components` when relevant.

This crate deliberately performs no network I/O, staging creation, finalization,
or overwrite operation. `plan_top_level_name` is planning only; production code
must reserve the chosen name with a no-replace filesystem primitive.
