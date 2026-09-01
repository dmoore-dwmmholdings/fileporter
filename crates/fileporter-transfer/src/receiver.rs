use crate::{plan_top_level_name, validate_receiver_components, TransferError};
use blake3::Hasher;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[cfg(test)]
thread_local! {
    // Fault injection is per test worker. Global flags made parallel tests
    // consume one another's faults, turning durable-write tests flaky.
    #[allow(clippy::missing_const_for_thread_local)] // The macro's MSRV-aware lint rejects the const form on this target.
    static FAIL_NEXT_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    #[allow(clippy::missing_const_for_thread_local)] // See FAIL_NEXT_WRITE; these are isolated test-worker fault switches.
    static FAIL_NEXT_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    #[allow(clippy::missing_const_for_thread_local)] // See FAIL_NEXT_WRITE; these are isolated test-worker fault switches.
    static FAIL_NEXT_FINALIZE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub fn fail_next_write_for_test() {
    FAIL_NEXT_WRITE.with(|value| value.set(true));
}
#[cfg(test)]
pub fn fail_next_sync_for_test() {
    FAIL_NEXT_SYNC.with(|value| value.set(true));
}
#[cfg(test)]
pub fn fail_next_finalize_for_test() {
    FAIL_NEXT_FINALIZE.with(|value| value.set(true));
}

pub struct StagingArea {
    receive_root: PathBuf,
    root: PathBuf,
    batch_id: Uuid,
}
impl StagingArea {
    pub fn create(receive_directory: &Path, batch_id: Uuid) -> Result<Self, TransferError> {
        let receive_root = receive_directory
            .canonicalize()
            .map_err(|_| TransferError::InvalidSource)?;
        if !receive_root.is_dir()
            || fs::symlink_metadata(&receive_root)
                .map_err(|_| TransferError::InvalidSource)?
                .file_type()
                .is_symlink()
        {
            return Err(TransferError::InvalidPath);
        }
        let parent = receive_root.join(".fileporter-staging");
        fs::create_dir_all(&parent).map_err(|_| TransferError::SourceMissing)?;
        let root = parent.join(batch_id.to_string());
        fs::create_dir(&root).map_err(|_| TransferError::InvalidPath)?;
        Ok(Self {
            receive_root,
            root,
            batch_id,
        })
    }
    /// Re-opens an app-owned staging directory for the same protocol batch.
    /// This is deliberately not a general path opener: the UUID-derived path
    /// remains beneath the canonical receive root.
    pub fn open(receive_directory: &Path, batch_id: Uuid) -> Result<Self, TransferError> {
        let receive_root = receive_directory
            .canonicalize()
            .map_err(|_| TransferError::InvalidSource)?;
        let root = receive_root
            .join(".fileporter-staging")
            .join(batch_id.to_string());
        if !root.is_dir()
            || fs::symlink_metadata(&root)
                .map_err(|_| TransferError::InvalidSource)?
                .file_type()
                .is_symlink()
        {
            return Err(TransferError::InvalidPath);
        }
        Ok(Self {
            receive_root,
            root,
            batch_id,
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn batch_id(&self) -> Uuid {
        self.batch_id
    }
    pub fn relative_path(&self, components: &[String]) -> Result<PathBuf, TransferError> {
        validate_receiver_components(components)?;
        let mut out = self.root.clone();
        for c in components {
            out.push(c)
        }
        if !out.starts_with(&self.root) {
            return Err(TransferError::InvalidPath);
        }
        Ok(out)
    }
    pub fn create_directories(&self, components: &[String]) -> Result<PathBuf, TransferError> {
        let p = self.relative_path(components)?;
        fs::create_dir_all(&p).map_err(|_| TransferError::SourceMissing)?;
        let mut cursor = self.root.clone();
        for c in components {
            cursor.push(c);
            if fs::symlink_metadata(&cursor)
                .map_err(|_| TransferError::SourceMissing)?
                .file_type()
                .is_symlink()
            {
                return Err(TransferError::InvalidPath);
            }
        }
        Ok(p)
    }
    pub fn cleanup_owned(self) -> Result<(), TransferError> {
        if self.root.parent() == Some(self.receive_root.join(".fileporter-staging").as_path()) {
            fs::remove_dir_all(&self.root).map_err(|_| TransferError::SourceMissing)?;
        }
        Ok(())
    }
}
pub fn enumerate_abandoned_staging(
    receive_directory: &Path,
) -> Result<Vec<PathBuf>, TransferError> {
    let root = receive_directory
        .canonicalize()
        .map_err(|_| TransferError::InvalidSource)?
        .join(".fileporter-staging");
    if !root.exists() {
        return Ok(vec![]);
    };
    fs::read_dir(root)
        .map_err(|_| TransferError::SourceMissing)?
        .map(|e| {
            e.map(|x| x.path())
                .map_err(|_| TransferError::SourceMissing)
        })
        .collect()
}

pub struct ReceiverFile {
    path: PathBuf,
    file: File,
    hasher: Hasher,
    offset: u64,
    expected_size: u64,
}
impl ReceiverFile {
    pub fn create(
        area: &StagingArea,
        components: &[String],
        expected_size: u64,
    ) -> Result<Self, TransferError> {
        let path = area.relative_path(components)?;
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).map_err(|_| TransferError::SourceMissing)?
        }
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| TransferError::InvalidPath)?;
        Ok(Self {
            path,
            file,
            hasher: Hasher::new(),
            offset: 0,
            expected_size,
        })
    }
    pub fn resume(
        area: &StagingArea,
        components: &[String],
        expected_size: u64,
        offset: u64,
    ) -> Result<Self, TransferError> {
        let path = area.relative_path(components)?;
        let mut prefix = File::open(&path).map_err(|_| TransferError::SourceMissing)?;
        if prefix
            .metadata()
            .map_err(|_| TransferError::SourceMissing)?
            .len()
            != offset
        {
            return Err(TransferError::InvalidPath);
        }
        let mut hasher = Hasher::new();
        let mut buf = [0u8; 1024 * 1024];
        loop {
            let n = prefix
                .read(&mut buf)
                .map_err(|_| TransferError::SourceMissing)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let file = OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|_| TransferError::SourceMissing)?;
        Ok(Self {
            path,
            file,
            hasher,
            offset,
            expected_size,
        })
    }
    pub fn offset(&self) -> u64 {
        self.offset
    }
    pub fn write_chunk(
        &mut self,
        offset: u64,
        bytes: &[u8],
        expected_hash: [u8; 32],
        checkpoint: impl FnOnce(u64) -> Result<(), TransferError>,
    ) -> Result<(), TransferError> {
        if bytes.len() > 1024 * 1024 || offset != self.offset {
            return Err(TransferError::InvalidPath);
        }
        if *blake3::hash(bytes).as_bytes() != expected_hash {
            return Err(TransferError::InvalidPath);
        }
        if self
            .offset
            .checked_add(bytes.len() as u64)
            .ok_or(TransferError::ManifestLimit)?
            > self.expected_size
        {
            return Err(TransferError::ManifestLimit);
        }
        #[cfg(test)]
        if FAIL_NEXT_WRITE.with(|value| value.replace(false)) {
            return Err(TransferError::DiskFull);
        }
        if self.file.write_all(bytes).is_err() || self.file.flush().is_err() {
            self.rollback_uncommitted_chunk(offset);
            return Err(TransferError::DiskFull);
        }
        #[cfg(test)]
        if FAIL_NEXT_SYNC.with(|value| value.replace(false)) {
            self.rollback_uncommitted_chunk(offset);
            return Err(TransferError::FsyncFailed);
        }
        if self.file.sync_data().is_err() {
            self.rollback_uncommitted_chunk(offset);
            return Err(TransferError::FsyncFailed);
        }
        self.hasher.update(bytes);
        self.offset += bytes.len() as u64;
        // The caller's durable database update is part of the acknowledgement
        // boundary.  Returning success here lets the network layer ACK only a
        // prefix that is both fsynced and recoverable after restart.
        checkpoint(self.offset)?;
        Ok(())
    }
    /// A failed write or fsync is not durable and must leave the staged file
    /// at the last acknowledged prefix, so retransmission at the same offset
    /// is safe after reconnect/restart. Best-effort cleanup does not change
    /// the returned failure or advance the in-memory/checkpoint offset.
    fn rollback_uncommitted_chunk(&mut self, offset: u64) {
        let _ = self.file.set_len(offset);
        let _ = self.file.sync_data();
    }
    pub fn complete(self, expected_hash: [u8; 32]) -> Result<PathBuf, TransferError> {
        self.file
            .sync_all()
            .map_err(|_| TransferError::SourceMissing)?;
        if self.offset != self.expected_size || *self.hasher.finalize().as_bytes() != expected_hash
        {
            return Err(TransferError::InvalidPath);
        }
        Ok(self.path)
    }
}
pub fn finalize_file_no_replace(
    staged: &Path,
    destination: &Path,
    original_name: &str,
) -> Result<PathBuf, TransferError> {
    #[cfg(test)]
    if FAIL_NEXT_FINALIZE.with(|value| value.replace(false)) {
        return Err(TransferError::FinalizeFailed);
    }
    let mut occupied = std::collections::HashSet::new();
    if destination.exists() {
        for e in fs::read_dir(destination).map_err(|_| TransferError::SourceMissing)? {
            occupied.insert(
                e.map_err(|_| TransferError::SourceMissing)?
                    .file_name()
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }
    for _ in 0..1000 {
        let name = plan_top_level_name(original_name, &occupied);
        let target = destination.join(&name);
        match fs::hard_link(staged, &target) {
            Ok(()) => {
                fs::remove_file(staged).map_err(|_| TransferError::SourceMissing)?;
                return Ok(target);
            }
            Err(_) => {
                occupied.insert(name);
            }
        }
    }
    Err(TransferError::ManifestLimit)
}

const VERIFIED_MARKER: &str = ".fileporter-verified-tree";

/// Writes an app-owned marker only after all descendants were verified by the caller.
pub fn mark_verified_tree(staged_directory: &Path) -> Result<(), TransferError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staged_directory.join(VERIFIED_MARKER))
        .map_err(|_| TransferError::InvalidPath)?
        .sync_all()
        .map_err(|_| TransferError::SourceMissing)
}

/// Applies the advertised modification time without making timestamp support a correctness gate.
pub fn apply_mtime_best_effort(path: &Path, unix_nanos: u128) {
    let seconds = (unix_nanos / 1_000_000_000).min(i64::MAX as u128) as i64;
    let nanos = (unix_nanos % 1_000_000_000) as u32;
    let _ = filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(seconds, nanos));
}

/// Finalizes a directory only after an explicit verified-tree marker. On Windows rename is
/// no-replace; other platforms use a create-new recursive fallback, so existing trees cannot
/// be overwritten or merged (but the fallback is not atomic).
pub fn finalize_verified_directory_no_replace(
    staged: &Path,
    destination: &Path,
    original_name: &str,
) -> Result<PathBuf, TransferError> {
    if !staged.is_dir() || !staged.join(VERIFIED_MARKER).is_file() {
        return Err(TransferError::InvalidPath);
    }
    let mut occupied = std::collections::HashSet::new();
    for e in fs::read_dir(destination).map_err(|_| TransferError::SourceMissing)? {
        occupied.insert(
            e.map_err(|_| TransferError::SourceMissing)?
                .file_name()
                .to_string_lossy()
                .to_string(),
        );
    }
    for _ in 0..1000 {
        let name = plan_top_level_name(original_name, &occupied);
        let target = destination.join(&name);
        #[cfg(windows)]
        {
            if fs::rename(staged, &target).is_ok() {
                return Ok(target);
            }
        }
        #[cfg(not(windows))]
        {
            if fs::create_dir(&target).is_ok() {
                if copy_tree_new(staged, &target).is_ok() {
                    fs::remove_dir_all(staged).map_err(|_| TransferError::SourceMissing)?;
                    return Ok(target);
                }
                let _ = fs::remove_dir_all(&target);
            }
        }
        occupied.insert(name);
    }
    Err(TransferError::ManifestLimit)
}
#[cfg(not(windows))]
fn copy_tree_new(source: &Path, target: &Path) -> Result<(), TransferError> {
    for entry in fs::read_dir(source).map_err(|_| TransferError::SourceMissing)? {
        let entry = entry.map_err(|_| TransferError::SourceMissing)?;
        let name = entry.file_name();
        if name == VERIFIED_MARKER {
            continue;
        }
        let from = entry.path();
        let to = target.join(name);
        let m = fs::symlink_metadata(&from).map_err(|_| TransferError::SourceMissing)?;
        if m.file_type().is_symlink() {
            return Err(TransferError::InvalidPath);
        }
        if m.is_dir() {
            fs::create_dir(&to).map_err(|_| TransferError::InvalidPath)?;
            copy_tree_new(&from, &to)?
        } else if m.is_file() {
            let mut r = File::open(from).map_err(|_| TransferError::SourceMissing)?;
            let mut w = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(to)
                .map_err(|_| TransferError::InvalidPath)?;
            std::io::copy(&mut r, &mut w).map_err(|_| TransferError::SourceMissing)?;
            w.sync_all().map_err(|_| TransferError::SourceMissing)?
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_resume_and_bad_chunk() {
        let d = tempfile::tempdir().unwrap();
        let a = StagingArea::create(d.path(), Uuid::new_v4()).unwrap();
        let c = vec!["zero".into()];
        let f = ReceiverFile::create(&a, &c, 0).unwrap();
        assert!(f.complete(*blake3::hash(b"").as_bytes()).is_ok());
        let c = vec!["x".into()];
        let mut f = ReceiverFile::create(&a, &c, 3).unwrap();
        assert!(f.write_chunk(0, b"abc", [0; 32], |_| Ok(())).is_err());
        let h = *blake3::hash(b"abc").as_bytes();
        f.write_chunk(0, b"abc", h, |_| Ok(())).unwrap();
        let p = f.complete(h).unwrap();
        assert_eq!(fs::read(p).unwrap(), b"abc");
    }
    #[test]
    fn finalization_preserves_existing() {
        let d = tempfile::tempdir().unwrap();
        let a = StagingArea::create(d.path(), Uuid::new_v4()).unwrap();
        let c = vec!["x".into()];
        let mut f = ReceiverFile::create(&a, &c, 1).unwrap();
        let h = *blake3::hash(b"a").as_bytes();
        f.write_chunk(0, b"a", h, |_| Ok(())).unwrap();
        let p = f.complete(h).unwrap();
        fs::write(d.path().join("Report.txt"), b"old").unwrap();
        let finalp = finalize_file_no_replace(&p, d.path(), "Report.txt").unwrap();
        assert_eq!(finalp.file_name().unwrap(), "Report (1).txt");
        assert_eq!(fs::read(d.path().join("Report.txt")).unwrap(), b"old");
    }
    #[test]
    fn resume_nonzero_prefix_and_mtime() {
        let d = tempfile::tempdir().unwrap();
        let a = StagingArea::create(d.path(), Uuid::new_v4()).unwrap();
        let c = vec!["r".into()];
        let mut f = ReceiverFile::create(&a, &c, 6).unwrap();
        let h = *blake3::hash(b"abc").as_bytes();
        f.write_chunk(0, b"abc", h, |_| Ok(())).unwrap();
        drop(f);
        let mut f = ReceiverFile::resume(&a, &c, 6, 3).unwrap();
        let h2 = *blake3::hash(b"def").as_bytes();
        f.write_chunk(3, b"def", h2, |_| Ok(())).unwrap();
        let p = f.complete(*blake3::hash(b"abcdef").as_bytes()).unwrap();
        apply_mtime_best_effort(&p, 1_700_000_000_123_000_000);
        assert!(p.metadata().unwrap().modified().is_ok());
    }
    #[test]
    fn restart_preserves_completed_earlier_entry_and_resumes_later_entry() {
        let d = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        let a = StagingArea::create(d.path(), id).unwrap();
        let first = vec!["tree".into(), "first".into()];
        let mut first_file = ReceiverFile::create(&a, &first, 1).unwrap();
        let first_hash = *blake3::hash(b"a").as_bytes();
        first_file
            .write_chunk(0, b"a", first_hash, |_| Ok(()))
            .unwrap();
        first_file.complete(first_hash).unwrap();
        let later = vec!["tree".into(), "later".into()];
        let mut later_file = ReceiverFile::create(&a, &later, 4).unwrap();
        let prefix_hash = *blake3::hash(b"ab").as_bytes();
        later_file
            .write_chunk(0, b"ab", prefix_hash, |_| Ok(()))
            .unwrap();
        drop(later_file);
        drop(a);
        let reopened = StagingArea::open(d.path(), id).unwrap();
        assert_eq!(
            fs::read(reopened.relative_path(&first).unwrap()).unwrap(),
            b"a"
        );
        let mut later_file = ReceiverFile::resume(&reopened, &later, 4, 2).unwrap();
        let suffix_hash = *blake3::hash(b"cd").as_bytes();
        later_file
            .write_chunk(2, b"cd", suffix_hash, |_| Ok(()))
            .unwrap();
        assert!(later_file
            .complete(*blake3::hash(b"abcd").as_bytes())
            .is_ok());
    }
    #[test]
    fn verified_directory_requires_marker_and_never_merges() {
        let d = tempfile::tempdir().unwrap();
        let a = StagingArea::create(d.path(), Uuid::new_v4()).unwrap();
        let tree = a.create_directories(&["tree".into()]).unwrap();
        fs::write(tree.join("new"), b"n").unwrap();
        assert!(finalize_verified_directory_no_replace(&tree, d.path(), "Photos").is_err());
        mark_verified_tree(&tree).unwrap();
        fs::create_dir(d.path().join("Photos")).unwrap();
        fs::write(d.path().join("Photos").join("old"), b"o").unwrap();
        let out = finalize_verified_directory_no_replace(&tree, d.path(), "Photos").unwrap();
        assert_eq!(out.file_name().unwrap(), "Photos (1)");
        assert_eq!(fs::read(d.path().join("Photos").join("old")).unwrap(), b"o");
        assert_eq!(fs::read(out.join("new")).unwrap(), b"n");
    }
    #[test]
    fn cleanup_is_scoped_to_owned_batch() {
        let d = tempfile::tempdir().unwrap();
        let a = StagingArea::create(d.path(), Uuid::new_v4()).unwrap();
        let owned = a.root().to_path_buf();
        fs::write(d.path().join("keep"), b"k").unwrap();
        a.cleanup_owned().unwrap();
        assert!(!owned.exists());
        assert!(d.path().join("keep").exists());
    }

    #[test]
    fn disk_full_and_fsync_faults_do_not_advance_the_durable_offset() {
        let d = tempfile::tempdir().unwrap();
        let a = StagingArea::create(d.path(), Uuid::new_v4()).unwrap();
        let c = vec!["fault.bin".into()];
        let mut file = ReceiverFile::create(&a, &c, 2).unwrap();
        fail_next_write_for_test();
        assert_eq!(
            file.write_chunk(0, b"a", *blake3::hash(b"a").as_bytes(), |_| Ok(())),
            Err(TransferError::DiskFull)
        );
        assert_eq!(file.offset(), 0);
        fail_next_sync_for_test();
        assert_eq!(
            file.write_chunk(0, b"a", *blake3::hash(b"a").as_bytes(), |_| Ok(())),
            Err(TransferError::FsyncFailed)
        );
        assert_eq!(file.offset(), 0);
    }

    #[test]
    fn finalize_fault_preserves_staging_and_collision_never_overwrites() {
        let d = tempfile::tempdir().unwrap();
        let a = StagingArea::create(d.path(), Uuid::new_v4()).unwrap();
        let c = vec!["new.bin".into()];
        let mut file = ReceiverFile::create(&a, &c, 1).unwrap();
        let hash = *blake3::hash(b"n").as_bytes();
        file.write_chunk(0, b"n", hash, |_| Ok(())).unwrap();
        let staged = file.complete(hash).unwrap();
        fail_next_finalize_for_test();
        assert_eq!(
            finalize_file_no_replace(&staged, d.path(), "same.bin"),
            Err(TransferError::FinalizeFailed)
        );
        assert!(staged.exists());
        fs::write(d.path().join("same.bin"), b"user").unwrap();
        let final_path = finalize_file_no_replace(&staged, d.path(), "same.bin").unwrap();
        assert_eq!(final_path.file_name().unwrap(), "same (1).bin");
        assert_eq!(fs::read(d.path().join("same.bin")).unwrap(), b"user");
    }
}
