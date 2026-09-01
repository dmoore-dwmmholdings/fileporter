//! Pure wake-gap policy for the bounded desktop network monitor.
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Desktop monitor consumes this; core tests exercise policy.
pub enum WakeAction {
    Reconcile,
    SuspendThenResume,
}
#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Desktop monitor consumes this; core tests exercise policy.
pub struct WakeGapDetector {
    last: Instant,
    gap: Duration,
}
impl WakeGapDetector {
    #[cfg_attr(not(feature = "desktop"), allow(dead_code))]
    pub fn new(now: Instant, interval: Duration) -> Self {
        Self {
            last: now,
            gap: interval.saturating_mul(2),
        }
    }
    #[cfg_attr(not(feature = "desktop"), allow(dead_code))]
    pub fn tick(&mut self, now: Instant) -> WakeAction {
        let action = if now.duration_since(self.last) > self.gap {
            WakeAction::SuspendThenResume
        } else {
            WakeAction::Reconcile
        };
        self.last = now;
        action
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normal_gap_and_idempotence() {
        let now = Instant::now();
        let mut detector = WakeGapDetector::new(now, Duration::from_secs(30));
        assert_eq!(
            detector.tick(now + Duration::from_secs(30)),
            WakeAction::Reconcile
        );
        assert_eq!(
            detector.tick(now + Duration::from_secs(100)),
            WakeAction::SuspendThenResume
        );
        assert_eq!(
            detector.tick(now + Duration::from_secs(130)),
            WakeAction::Reconcile
        );
    }
}
