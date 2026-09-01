//! Notification dispatch is ledger-backed: terminal incoming history is
//! claimed once before a platform notification is attempted, so restarts never
//! replay old transfer alerts. Text intentionally omits names and paths.

use crate::{
    desktop_actions::incoming_notification_text, error::AppError, persistence::SettingsRepository,
};

pub trait Notifier {
    fn send(&self, title: &str, body: &str) -> Result<(), AppError>;
}

pub fn dispatch_terminal_incoming<N: Notifier>(
    repository: &SettingsRepository,
    notifier: &N,
    now: i64,
) -> Result<usize, AppError> {
    let settings = repository.load()?;
    if !settings.notifications_enabled {
        return Ok(0);
    }
    let mut sent = 0;
    for record in repository.all_batches()?.into_iter().filter(|record| {
        record.batch.direction == "incoming"
            && matches!(
                record.batch.state.as_str(),
                "completed" | "failed" | "cancelled"
            )
    }) {
        if !repository.claim_incoming_notification(&record.batch.id, now)? {
            continue;
        }
        let (title, body) = incoming_notification_text(
            record.batch.state == "completed",
            record.batch.total_entries.max(0) as usize,
        );
        notifier.send(title, &body)?;
        sent += 1;
    }
    Ok(sent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{Batch, SettingsRepository};
    use std::sync::Mutex;
    struct TestNotifier(Mutex<Vec<(String, String)>>);
    impl Notifier for TestNotifier {
        fn send(&self, title: &str, body: &str) -> Result<(), AppError> {
            self.0.lock().unwrap().push((title.into(), body.into()));
            Ok(())
        }
    }
    struct FailingNotifier;
    impl Notifier for FailingNotifier {
        fn send(&self, _: &str, _: &str) -> Result<(), AppError> {
            Err(AppError::DesktopActionFailed)
        }
    }
    fn terminal_batch(id: &str) -> Batch {
        Batch {
            id: id.into(),
            direction: "incoming".into(),
            state: "completed".into(),
            created_at: 1,
            completed_at: Some(2),
            total_bytes: 1,
            total_entries: 2,
            warning_count: 0,
            wait_for_available: false,
        }
    }
    #[test]
    fn terminal_incoming_is_redacted_and_only_notified_once() {
        let dir = tempfile::tempdir().unwrap();
        let repo = SettingsRepository::open(dir.path().join("state.sqlite")).unwrap();
        repo.save_batch(&terminal_batch("in")).unwrap();
        let notifier = TestNotifier(Mutex::new(Vec::new()));
        assert_eq!(dispatch_terminal_incoming(&repo, &notifier, 3).unwrap(), 1);
        assert_eq!(dispatch_terminal_incoming(&repo, &notifier, 4).unwrap(), 0);
        let body = &notifier.0.lock().unwrap()[0].1;
        assert_eq!(body, "Received 2 item(s).");
    }
    #[test]
    fn disabled_notifications_never_claim_or_send() {
        let dir = tempfile::tempdir().unwrap();
        let repo = SettingsRepository::open(dir.path().join("state.sqlite")).unwrap();
        repo.save_batch(&terminal_batch("in")).unwrap();
        let mut settings = repo.load().unwrap();
        settings.notifications_enabled = false;
        repo.save(&settings).unwrap();
        let notifier = TestNotifier(Mutex::new(Vec::new()));
        assert_eq!(dispatch_terminal_incoming(&repo, &notifier, 3).unwrap(), 0);
        assert!(notifier.0.lock().unwrap().is_empty());
        assert!(repo.claim_incoming_notification("in", 4).unwrap());
    }
    #[test]
    fn failed_delivery_is_redacted_and_not_replayed_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        let repo = SettingsRepository::open(path.clone()).unwrap();
        repo.save_batch(&terminal_batch("in-sensitive-path"))
            .unwrap();
        assert!(dispatch_terminal_incoming(&repo, &FailingNotifier, 3).is_err());
        drop(repo);
        let reopened = SettingsRepository::open(path).unwrap();
        let notifier = TestNotifier(Mutex::new(Vec::new()));
        assert_eq!(
            dispatch_terminal_incoming(&reopened, &notifier, 4).unwrap(),
            0
        );
        let (_, body) = incoming_notification_text(false, 1);
        assert_eq!(body, "An incoming transfer could not be completed.");
        assert!(!body.contains("sensitive"));
    }
}
