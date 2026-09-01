use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration, SystemTime},
};

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Desktop bootstrap keeps the non-blocking writer alive.
static LOG_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

/// Seven daily files are normally far below this cap; the cap also protects a
/// verbose diagnostic session from consuming unbounded app-local storage.
pub const LOG_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const MAX_LOG_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct OwnedLogFile {
    path: PathBuf,
    modified: SystemTime,
    bytes: u64,
}

#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Called only by the desktop bootstrap.
pub fn initialize(log_directory: &std::path::Path) {
    let _ = std::fs::create_dir_all(log_directory);
    cleanup(log_directory, SystemTime::now());
    let appender = tracing_appender::rolling::daily(log_directory, "fileporter.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let _ = LOG_GUARD.set(guard);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("fileporter_lib=info,tao=warn,wry=warn"));
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer),
        )
        .try_init()
        .ok();
    tracing::info!(
        event_code = "logging.initialized",
        "local structured logging initialized"
    );
}

/// Deletes only regular Fileporter rolling log files. Files owned by users or
/// other software in this directory are intentionally ignored.
pub fn cleanup(log_directory: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(log_directory) else {
        return;
    };
    let files = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if !(name == "fileporter.log" || name.starts_with("fileporter.log.")) {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| OwnedLogFile {
                path: entry.path(),
                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                bytes: metadata.len(),
            })
        })
        .collect();
    for file in files_to_remove(files, now) {
        let _ = fs::remove_file(file.path);
    }
}

fn files_to_remove(mut files: Vec<OwnedLogFile>, now: SystemTime) -> Vec<OwnedLogFile> {
    let mut remove = Vec::new();
    files.retain(|file| {
        let expired = now
            .duration_since(file.modified)
            .map(|age| age > LOG_RETENTION)
            .unwrap_or(false);
        if expired {
            remove.push(file.clone());
        }
        !expired
    });
    files.sort_by_key(|file| file.modified);
    let mut total: u64 = files.iter().map(|file| file.bytes).sum();
    for file in files {
        if total <= MAX_LOG_BYTES {
            break;
        }
        total = total.saturating_sub(file.bytes);
        remove.push(file);
    }
    remove
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_selection_keeps_recent_files_within_the_cap() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let old = OwnedLogFile {
            path: "old".into(),
            modified: now - LOG_RETENTION - Duration::from_secs(1),
            bytes: 1,
        };
        let first = OwnedLogFile {
            path: "first".into(),
            modified: now - Duration::from_secs(3),
            bytes: 20 * 1024 * 1024,
        };
        let second = OwnedLogFile {
            path: "second".into(),
            modified: now - Duration::from_secs(2),
            bytes: 20 * 1024 * 1024,
        };
        let removed = files_to_remove(vec![second.clone(), old.clone(), first.clone()], now);
        assert_eq!(
            removed.iter().map(|file| &file.path).collect::<Vec<_>>(),
            vec![&old.path, &first.path]
        );
    }

    #[test]
    fn cleanup_never_touches_unowned_files() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("notes.txt"), b"keep").unwrap();
        fs::write(directory.path().join("fileporter.log"), b"keep").unwrap();
        cleanup(directory.path(), SystemTime::now());
        assert!(directory.path().join("notes.txt").exists());
        assert!(directory.path().join("fileporter.log").exists());
    }
}
