//! Reconciles persisted receiving preferences with the runtime listener.
//!
//! This intentionally manages only socket ownership. It neither discovers
//! peers nor schedules transfers.

use std::net::SocketAddr;

use crate::engine::{validate_listen_address, ListenerError, ListenerStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerAction {
    Noop,
    Start(SocketAddr),
    Stop,
    Restart(SocketAddr),
}

/// Small pure coordinator that makes lifecycle policy independently testable.
pub struct ListenerLifecycleCoordinator;

impl ListenerLifecycleCoordinator {
    pub fn action(
        onboarding_complete: bool,
        receiving_enabled: bool,
        shutting_down: bool,
        configured_address: &str,
        status: ListenerStatus,
    ) -> Result<ListenerAction, ListenerError> {
        let should_listen = onboarding_complete && receiving_enabled && !shutting_down;
        if !should_listen {
            return Ok(if status.listening {
                ListenerAction::Stop
            } else {
                ListenerAction::Noop
            });
        }

        let address = validate_listen_address(configured_address)?;
        Ok(match status.bound_endpoint {
            // Port zero persists an OS-assigned-port request, not a literal
            // endpoint. Once started, any socket bound on that configured IP
            // satisfies it until settings change or receiving is disabled.
            Some(bound)
                if bound == address || (address.port() == 0 && bound.ip() == address.ip()) =>
            {
                ListenerAction::Noop
            }
            Some(_) => ListenerAction::Restart(address),
            None => ListenerAction::Start(address),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stopped() -> ListenerStatus {
        ListenerStatus {
            listening: false,
            receiving: false,
            bound_endpoint: None,
        }
    }

    #[test]
    fn onboarding_and_preference_gate_listener_start() {
        assert_eq!(
            ListenerLifecycleCoordinator::action(false, true, false, "127.0.0.1:4242", stopped())
                .unwrap(),
            ListenerAction::Noop
        );
        assert_eq!(
            ListenerLifecycleCoordinator::action(true, false, false, "127.0.0.1:4242", stopped())
                .unwrap(),
            ListenerAction::Noop
        );
        assert_eq!(
            ListenerLifecycleCoordinator::action(true, true, false, "127.0.0.1:4242", stopped())
                .unwrap(),
            ListenerAction::Start("127.0.0.1:4242".parse().unwrap())
        );
    }

    #[test]
    fn coordinator_stops_or_restarts_only_when_needed() {
        let running = ListenerStatus {
            listening: true,
            receiving: false,
            bound_endpoint: Some("127.0.0.1:4242".parse().unwrap()),
        };
        assert_eq!(
            ListenerLifecycleCoordinator::action(true, false, false, "127.0.0.1:4242", running)
                .unwrap(),
            ListenerAction::Stop
        );
        assert_eq!(
            ListenerLifecycleCoordinator::action(true, true, false, "127.0.0.2:0", running)
                .unwrap(),
            ListenerAction::Restart("127.0.0.2:0".parse().unwrap())
        );
    }

    #[test]
    fn ephemeral_port_is_stable_after_binding_and_shutdown_stops_it() {
        let running = ListenerStatus {
            listening: true,
            receiving: false,
            bound_endpoint: Some("127.0.0.1:51342".parse().unwrap()),
        };
        assert_eq!(
            ListenerLifecycleCoordinator::action(true, true, false, "127.0.0.1:0", running)
                .unwrap(),
            ListenerAction::Noop
        );
        assert_eq!(
            ListenerLifecycleCoordinator::action(true, true, true, "127.0.0.1:0", running).unwrap(),
            ListenerAction::Stop
        );
    }
}
