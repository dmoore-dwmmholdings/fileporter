# ADR 0002: Durable staging and no-overwrite finalization

Receive work is staged inside an app-owned receive-directory subtree. Checkpoints represent durable byte offsets, and a completed entry is hash-verified before finalization. Finalization never replaces an existing destination entry.

This favors preservation over convenience: crashes leave recoverable staging data, and a collision is reported instead of silently overwriting a user's file.
