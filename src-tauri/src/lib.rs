#[cfg(feature = "desktop")]
mod app;
#[cfg(feature = "desktop")]
mod commands;
#[cfg(any(feature = "desktop", feature = "mobile", test))]
mod desktop_actions;
#[cfg(any(feature = "desktop", feature = "mobile", test))]
mod desktop_notifications;
#[cfg(any(feature = "desktop", feature = "mobile", test))]
mod discovery;
#[cfg(all(target_vendor = "apple", any(feature = "mobile", test)))]
mod discovery_dnssd;
mod engine;
mod error;
mod identity;
mod lifecycle_monitor;
#[cfg(any(feature = "desktop", feature = "mobile", test))]
mod listener_lifecycle;
#[cfg(any(feature = "desktop", feature = "mobile", test))]
mod logging;
#[cfg(feature = "mobile")]
pub mod mobile;
mod persistence;
mod secret_store;
#[cfg(any(feature = "desktop", feature = "mobile", test))]
mod settings_ops;
#[cfg(any(feature = "desktop", feature = "mobile", test))]
mod state;
mod state_events;
#[cfg(feature = "desktop")]
mod tray;

#[cfg(feature = "desktop")]
pub use app::run;
pub use engine::{
    is_loopback_or_private, validate_listen_address, validate_manual_endpoint, Engine,
    EngineLifecycle, ListenerError, ListenerStatus,
};
pub use persistence::{
    Batch, BatchTarget, Checkpoint, Settings, SettingsRepository, TransferItem, TrustedPeer,
};
