use blake3::Hasher;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

mod receiver;
pub use receiver::{
    apply_mtime_best_effort, enumerate_abandoned_staging, finalize_file_no_replace,
    finalize_verified_directory_no_replace, mark_verified_tree, ReceiverFile, StagingArea,
};

pub const MAX_DEPTH: usize = 256;
pub const MAX_COMPONENT_BYTES: usize = 255;
pub const MAX_PATH_BYTES: usize = 32 * 1024;
pub const MAX_ENTRIES: usize = 100_000;
/// A manifest is metadata, but its advertised logical length still needs a
/// ceiling so an offer cannot overflow persistence/progress accounting.
pub const MAX_TOTAL_LOGICAL_BYTES: u64 = i64::MAX as u64;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransferError {
    #[error("cancelled")]
    Cancelled,
    #[error("invalid source path")]
    InvalidSource,
    #[error("source missing")]
    SourceMissing,
    #[error("manifest limit exceeded")]
    ManifestLimit,
    #[error("invalid path component")]
    InvalidPath,
    #[error("durable checkpoint could not be recorded")]
    Durability,
    #[error("destination storage is full")]
    DiskFull,
    #[error("staged data could not be flushed")]
    FsyncFailed,
    #[error("finalization failed")]
    FinalizeFailed,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceFingerprint {
    pub size: u64,
    pub modified_unix_nanos: u128,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestEntry {
    pub id: String,
    pub kind: EntryKind,
    pub components: Vec<String>,
    pub size: u64,
    pub modified_unix_nanos: u128,
    #[serde(skip_serializing)]
    pub source_path_local: PathBuf,
    pub fingerprint: SourceFingerprint,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestWarning {
    pub code: String,
    pub components: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceManifest {
    pub entries: Vec<ManifestEntry>,
    pub warnings: Vec<ManifestWarning>,
    pub total_logical_bytes: u64,
    pub total_entries: usize,
}

pub fn build_source_manifest(
    paths: Vec<PathBuf>,
    cancellation: &CancellationToken,
) -> Result<SourceManifest, TransferError> {
    let roots = normalize_roots(paths)?;
    let mut result = SourceManifest {
        entries: Vec::new(),
        warnings: Vec::new(),
        total_logical_bytes: 0,
        total_entries: 0,
    };
    for root in roots {
        let top = root
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or(TransferError::InvalidSource)?
            .to_owned();
        scan(&root, vec![top], cancellation, &mut result)?;
    }
    result
        .entries
        .sort_by(|a, b| a.components.cmp(&b.components));
    // Two independently selected roots can have the same leaf name.  There is
    // no safe implicit merge for that case: a receiver must be able to map each
    // advertised relative path to exactly one source entry.
    if result
        .entries
        .windows(2)
        .any(|pair| pair[0].components == pair[1].components)
    {
        return Err(TransferError::InvalidSource);
    }
    Ok(result)
}
fn normalize_roots(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, TransferError> {
    let mut roots = BTreeSet::new();
    for path in paths {
        if !path.is_absolute() {
            return Err(TransferError::InvalidSource);
        }
        let canonical = path
            .canonicalize()
            .map_err(|_| TransferError::SourceMissing)?;
        roots.insert(canonical);
    }
    let all_roots = roots.clone();
    Ok(roots
        .into_iter()
        .filter(|candidate| {
            !all_roots
                .iter()
                .any(|other| other != candidate && candidate.starts_with(other))
        })
        .collect())
}
fn scan(
    path: &Path,
    components: Vec<String>,
    cancellation: &CancellationToken,
    manifest: &mut SourceManifest,
) -> Result<(), TransferError> {
    if cancellation.is_cancelled() {
        return Err(TransferError::Cancelled);
    }
    validate_receiver_components(&components)?;
    if manifest.entries.len() >= MAX_ENTRIES {
        return Err(TransferError::ManifestLimit);
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| TransferError::SourceMissing)?;
    if metadata.file_type().is_symlink() {
        manifest.warnings.push(ManifestWarning {
            code: "unsupported_entry".into(),
            components,
        });
        return Ok(());
    }
    let modified = modified_nanos(&metadata);
    let fingerprint = SourceFingerprint {
        size: metadata.len(),
        modified_unix_nanos: modified,
    };
    if metadata.is_file() {
        let id = entry_id(&components, &fingerprint);
        manifest.total_logical_bytes = manifest
            .total_logical_bytes
            .checked_add(metadata.len())
            .filter(|total| *total <= MAX_TOTAL_LOGICAL_BYTES)
            .ok_or(TransferError::ManifestLimit)?;
        manifest.entries.push(ManifestEntry {
            id,
            kind: EntryKind::File,
            components,
            size: metadata.len(),
            modified_unix_nanos: modified,
            source_path_local: path.to_path_buf(),
            fingerprint,
        });
        manifest.total_entries += 1;
        return Ok(());
    }
    if !metadata.is_dir() {
        manifest.warnings.push(ManifestWarning {
            code: "unsupported_entry".into(),
            components,
        });
        return Ok(());
    }
    let id = entry_id(&components, &fingerprint);
    manifest.entries.push(ManifestEntry {
        id,
        kind: EntryKind::Directory,
        components: components.clone(),
        size: 0,
        modified_unix_nanos: modified,
        source_path_local: path.to_path_buf(),
        fingerprint,
    });
    manifest.total_entries += 1;
    let mut children = fs::read_dir(path)
        .map_err(|_| TransferError::SourceMissing)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| TransferError::SourceMissing)?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let name = child.file_name().to_string_lossy().to_string();
        let mut child_components = components.clone();
        child_components.push(name);
        scan(&child.path(), child_components, cancellation, manifest)?;
    }
    Ok(())
}
fn modified_nanos(metadata: &fs::Metadata) -> u128 {
    metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
fn entry_id(components: &[String], fingerprint: &SourceFingerprint) -> String {
    let mut hasher = Hasher::new();
    for c in components {
        hasher.update(c.as_bytes());
        hasher.update(&[0]);
    }
    hasher.update(&fingerprint.size.to_be_bytes());
    hasher.update(&fingerprint.modified_unix_nanos.to_be_bytes());
    hasher.finalize().to_hex()[..16].to_string()
}
pub fn source_unchanged(entry: &ManifestEntry) -> Result<bool, TransferError> {
    let metadata =
        fs::metadata(&entry.source_path_local).map_err(|_| TransferError::SourceMissing)?;
    Ok(SourceFingerprint {
        size: metadata.len(),
        modified_unix_nanos: modified_nanos(&metadata),
    } == entry.fingerprint)
}

pub fn validate_receiver_components(components: &[String]) -> Result<(), TransferError> {
    if components.is_empty() || components.len() > MAX_DEPTH {
        return Err(TransferError::InvalidPath);
    }
    let mut total = 0;
    for component in components {
        let bytes = component.as_bytes();
        total += bytes.len();
        if bytes.is_empty()
            || bytes.len() > MAX_COMPONENT_BYTES
            || total > MAX_PATH_BYTES
            || component == "."
            || component == ".."
            || component.contains('\0')
            || component.contains('/')
            || component.contains('\\')
            || component.starts_with("//")
            || component.starts_with("\\\\")
            || has_drive_prefix(component)
        {
            return Err(TransferError::InvalidPath);
        }
    }
    Ok(())
}
fn has_drive_prefix(value: &str) -> bool {
    value.as_bytes().get(1) == Some(&b':')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
}
pub fn sanitize_windows_components(
    components: &[String],
) -> Result<(Vec<String>, Vec<ManifestWarning>), TransferError> {
    validate_receiver_components(components)?;
    let mut used = HashSet::new();
    let mut out = Vec::with_capacity(components.len());
    let mut warnings = Vec::new();
    for component in components {
        let mut candidate = windows_base(component);
        let base_key = candidate.to_ascii_lowercase();
        let collision = !used.insert(base_key);
        let changed = candidate != *component || collision;
        if changed {
            candidate = suffix_component(&candidate, component);
            while !used.insert(candidate.to_ascii_lowercase()) {
                candidate = suffix_component(&candidate, &(component.clone() + "~"));
            }
            warnings.push(ManifestWarning {
                code: "windows_name_sanitized".into(),
                components: vec![component.clone()],
            });
        }
        out.push(candidate);
    }
    Ok((out, warnings))
}
fn windows_base(original: &str) -> String {
    let mut value: String = original
        .chars()
        .map(|c| {
            if c <= '\u{1f}' || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    value = value.trim_end_matches(['.', ' ']).to_owned();
    if value.is_empty() || reserved_dos_name(&value) {
        value = format!("_{value}");
    }
    value
}
fn reserved_dos_name(value: &str) -> bool {
    let stem = value.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}
fn suffix_component(base: &str, original: &str) -> String {
    let hash = blake3::hash(original.as_bytes()).to_hex()[..8].to_string();
    match base.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}~{hash}.{ext}"),
        _ => format!("{base}~{hash}"),
    }
}
pub fn plan_top_level_name(original: &str, occupied: &HashSet<String>) -> String {
    if !occupied.contains(original) {
        return original.to_owned();
    }
    let (stem, extension) = match original.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (original, String::new()),
    };
    let mut number = 1;
    loop {
        let candidate = format!("{stem} ({number}){extension}");
        if !occupied.contains(&candidate) {
            return candidate;
        }
        number += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn token() -> CancellationToken {
        CancellationToken::new()
    }
    #[test]
    fn manifest_preserves_empty_unicode_hidden_and_nested() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        fs::create_dir_all(root.join("empty")).unwrap();
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("nested").join("héllo.txt"), b"x").unwrap();
        fs::write(root.join(".hidden"), b"h").unwrap();
        let m = build_source_manifest(vec![root], &token()).unwrap();
        assert!(m
            .entries
            .iter()
            .any(|e| e.components.ends_with(&["empty".into()])));
        assert!(m
            .entries
            .iter()
            .any(|e| e.components.ends_with(&["héllo.txt".into()])));
        assert!(m
            .entries
            .iter()
            .any(|e| e.components.ends_with(&[".hidden".into()])));
    }
    #[test]
    fn nested_duplicate_root_is_removed() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path().join("root");
        fs::create_dir_all(r.join("child")).unwrap();
        let m = build_source_manifest(vec![r.clone(), r.join("child")], &token()).unwrap();
        assert_eq!(
            m.entries.iter().filter(|e| e.components.len() == 1).count(),
            1
        );
    }
    #[test]
    fn independently_selected_same_named_roots_do_not_merge() {
        let d = tempfile::tempdir().unwrap();
        let left = d.path().join("left").join("report");
        let right = d.path().join("right").join("report");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        assert_eq!(
            build_source_manifest(vec![left, right], &token()),
            Err(TransferError::InvalidSource)
        );
    }
    #[test]
    fn receiver_rejects_traversal_and_absolute_components() {
        for v in [
            vec!["..".into()],
            vec!["a/b".into()],
            vec!["C:".into()],
            vec!["\\\\server".into()],
        ] {
            assert!(validate_receiver_components(&v).is_err());
        }
    }
    #[test]
    fn windows_policy_sanitizes_reserved_and_collisions() {
        let (v, w) =
            sanitize_windows_components(&["CON".into(), "a<b.txt".into(), "A_B.txt".into()])
                .unwrap();
        assert!(v[0].starts_with("_CON~"));
        assert!(v[1].contains('~'));
        assert!(v[2].contains('~'));
        assert_eq!(w.len(), 3);
    }
    #[test]
    fn top_level_numbering_never_reuses_name() {
        let occupied = HashSet::from(["Report.pdf".into(), "Report (1).pdf".into()]);
        assert_eq!(
            plan_top_level_name("Report.pdf", &occupied),
            "Report (2).pdf"
        );
    }
    #[test]
    fn mutation_changes_fingerprint() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        fs::write(&p, b"a").unwrap();
        let m = build_source_manifest(vec![p.clone()], &token()).unwrap();
        fs::write(p, b"longer").unwrap();
        assert!(!source_unchanged(&m.entries[0]).unwrap());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_is_recorded_as_unsupported_without_following() {
        use std::os::unix::fs::symlink;
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        fs::create_dir(&root).unwrap();
        fs::write(d.path().join("outside"), b"secret").unwrap();
        symlink(d.path().join("outside"), root.join("link")).unwrap();
        let manifest = build_source_manifest(vec![root], &token()).unwrap();
        assert!(manifest
            .warnings
            .iter()
            .any(|w| w.components.ends_with(&["link".into()])));
        assert!(!manifest
            .entries
            .iter()
            .any(|e| e.components.ends_with(&["link".into()])));
    }
}
