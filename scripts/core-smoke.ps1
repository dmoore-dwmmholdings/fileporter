$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
  cargo test --manifest-path src-tauri/Cargo.toml --lib --no-default-features engine::tests::two_auto_enabled_peers_trust_after_identity_proof_without_user_confirmation
  cargo test --manifest-path src-tauri/Cargo.toml --lib --no-default-features engine::tests::automatic_pairing_rejects_an_endpoint_with_a_different_discovered_identity
  cargo test --manifest-path src-tauri/Cargo.toml --lib --no-default-features engine::tests::two_peers_commit_trust_only_after_both_confirm_over_listener
  cargo test --manifest-path src-tauri/Cargo.toml --lib --no-default-features discovery::tests::advertise_update_expiry_spoof_and_restart_are_deterministic
  cargo test --manifest-path src-tauri/Cargo.toml --lib --no-default-features state::tests::scheduler_batch_expands_multiple_picker_files_and_directory
  cargo test --manifest-path src-tauri/Cargo.toml --lib --no-default-features engine::tests::persistent_two_peer_resume_after_disconnect_finalizes_once
  cargo test --manifest-path src-tauri/Cargo.toml --lib --no-default-features state::tests::offline_policy_persists_waiting_state_and_presence_wake_clears_backoff
  cargo test --manifest-path src-tauri/Cargo.toml --lib --no-default-features state::tests::fanout_keeps_successful_sibling_completed_when_other_target_fails
} finally { Pop-Location }
