//! Native desktop actions are deliberately kept behind a small adapter.  The
//! webview supplies only durable IDs; this module resolves those IDs to known,
//! completed incoming outputs before any platform API is reached.

use crate::{error::AppError, persistence::SettingsRepository};
use std::path::{Path, PathBuf};

/// Resolve only locally-owned finalized incoming top-level outputs.  Paths are
/// read from persistence, canonicalized, and verified to still exist; callers
/// can never influence a filesystem path through this API.
pub fn completed_output_for_item(
    repository: &SettingsRepository,
    item_id: &str,
) -> Result<PathBuf, AppError> {
    let records = repository.all_batches()?;
    let item = records
        .iter()
        .filter(|record| record.batch.direction == "incoming" && record.batch.state == "completed")
        .flat_map(|record| record.items.iter())
        .find(|item| {
            item.id == item_id && item.parent_item_id.is_none() && item.state == "completed"
        })
        .ok_or(AppError::CompletedOutputUnavailable)?;
    let path = item
        .destination_path_local
        .as_deref()
        .ok_or(AppError::CompletedOutputUnavailable)?;
    let canonical = Path::new(path)
        .canonicalize()
        .map_err(|_| AppError::CompletedOutputUnavailable)?;
    if !canonical.exists() {
        return Err(AppError::CompletedOutputUnavailable);
    }
    Ok(canonical)
}

#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Tauri commands call this in production; no-default tests cover path authorization separately.
pub fn completed_outputs_for_batch(
    repository: &SettingsRepository,
    batch_id: &str,
) -> Result<Vec<PathBuf>, AppError> {
    let record = repository
        .all_batches()?
        .into_iter()
        .find(|record| {
            record.batch.id == batch_id
                && record.batch.direction == "incoming"
                && record.batch.state == "completed"
        })
        .ok_or(AppError::CompletedOutputUnavailable)?;
    let mut outputs = record
        .items
        .iter()
        .filter(|item| item.parent_item_id.is_none() && item.state == "completed")
        .map(|item| completed_output_for_item(repository, &item.id))
        .collect::<Result<Vec<_>, _>>()?;
    outputs.sort();
    if outputs.is_empty() {
        return Err(AppError::CompletedOutputUnavailable);
    }
    Ok(outputs)
}

/// Privacy-safe notification text deliberately omits peer names and paths.
pub fn incoming_notification_text(completed: bool, item_count: usize) -> (&'static str, String) {
    if completed {
        ("Fileporter", format!("Received {item_count} item(s)."))
    } else {
        (
            "Fileporter",
            "An incoming transfer could not be completed.".into(),
        )
    }
}

/// Reveal through a direct process invocation. Paths are durable-ID resolved;
/// no shell command string is constructed.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Invoked only by desktop commands.
pub fn reveal_native(paths: &[PathBuf]) -> Result<(), AppError> {
    for path in paths {
        #[cfg(target_os = "windows")]
        let status = std::process::Command::new("explorer.exe")
            .arg(format!("/select,{}", path.display()))
            .status();
        #[cfg(target_os = "macos")]
        let status = std::process::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(path)
            .status();
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let status: std::io::Result<std::process::ExitStatus> = Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "desktop reveal",
        ));
        if !status.map(|value| value.success()).unwrap_or(false) {
            return Err(AppError::DesktopActionFailed);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Desktop command input; no-default tests assert its payload mapping below.
pub enum FileClipboardOperation {
    Copy,
    Move,
}

impl FileClipboardOperation {
    #[cfg(target_os = "windows")]
    const fn drop_effect(self) -> u32 {
        match self {
            Self::Copy => 1, // DROPEFFECT_COPY
            Self::Move => 2, // DROPEFFECT_MOVE
        }
    }
}

/// Windows public CF_HDROP clipboard data. Move advertises the standard
/// `Preferred DropEffect`; Fileporter never deletes a source itself.
#[cfg(target_os = "windows")]
#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Invoked only by desktop commands.
pub fn copy_native(paths: &[PathBuf]) -> Result<(), AppError> {
    set_windows_file_clipboard(paths, FileClipboardOperation::Copy)
}

#[cfg(target_os = "windows")]
#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Invoked only by desktop commands.
pub fn move_native(paths: &[PathBuf]) -> Result<(), AppError> {
    set_windows_file_clipboard(paths, FileClipboardOperation::Move)
}

#[cfg(target_os = "windows")]
fn set_windows_file_clipboard(
    paths: &[PathBuf],
    operation: FileClipboardOperation,
) -> Result<(), AppError> {
    #[repr(C)]
    struct DropFiles {
        p_files: u32,
        pt_x: i32,
        pt_y: i32,
        f_nc: i32,
        f_wide: i32,
    }
    #[allow(dead_code)] // These ABI declarations are reached only through reveal/copy desktop commands.
    unsafe extern "system" {
        fn OpenClipboard(owner: *mut core::ffi::c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(format: u32, handle: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
        fn GlobalAlloc(flags: u32, bytes: usize) -> *mut core::ffi::c_void;
        fn GlobalLock(handle: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
        fn GlobalUnlock(handle: *mut core::ffi::c_void) -> i32;
        fn GlobalFree(handle: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
    }
    const CF_HDROP: u32 = 15;
    const GMEM_MOVEABLE: u32 = 0x0002;
    use std::os::windows::ffi::OsStrExt;
    let mut names = Vec::<u16>::new();
    for path in paths {
        names.extend(path.as_os_str().encode_wide());
        names.push(0);
    }
    names.push(0);
    let bytes = std::mem::size_of::<DropFiles>() + names.len() * 2;
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err(AppError::ClipboardBusy);
        }
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if handle.is_null() || EmptyClipboard() == 0 {
            if !handle.is_null() {
                GlobalFree(handle);
            }
            CloseClipboard();
            return Err(AppError::DesktopActionFailed);
        }
        let memory = GlobalLock(handle);
        if memory.is_null() {
            GlobalFree(handle);
            CloseClipboard();
            return Err(AppError::DesktopActionFailed);
        }
        (memory as *mut DropFiles).write(DropFiles {
            p_files: std::mem::size_of::<DropFiles>() as u32,
            pt_x: 0,
            pt_y: 0,
            f_nc: 0,
            f_wide: 1,
        });
        std::ptr::copy_nonoverlapping(
            names.as_ptr() as *const u8,
            (memory as *mut u8).add(std::mem::size_of::<DropFiles>()),
            names.len() * 2,
        );
        GlobalUnlock(handle);
        let files_accepted = !SetClipboardData(CF_HDROP, handle).is_null();
        if !files_accepted {
            GlobalFree(handle);
            CloseClipboard();
            return Err(AppError::DesktopActionFailed);
        }
        let effect_name: Vec<u16> = "Preferred DropEffect\0".encode_utf16().collect();
        let effect_format = RegisterClipboardFormatW(effect_name.as_ptr());
        let effect_handle = GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of::<u32>());
        if effect_format == 0 || effect_handle.is_null() {
            if !effect_handle.is_null() {
                GlobalFree(effect_handle);
            }
            CloseClipboard();
            return Err(AppError::DesktopActionFailed);
        }
        let effect_memory = GlobalLock(effect_handle);
        if effect_memory.is_null() {
            GlobalFree(effect_handle);
            CloseClipboard();
            return Err(AppError::DesktopActionFailed);
        }
        (effect_memory as *mut u32).write(windows_drop_effect_payload(operation));
        GlobalUnlock(effect_handle);
        let effect_accepted = !SetClipboardData(effect_format, effect_handle).is_null();
        if !effect_accepted {
            GlobalFree(effect_handle);
        }
        CloseClipboard();
        effect_accepted
            .then_some(())
            .ok_or(AppError::DesktopActionFailed)
    }
}

#[cfg(target_os = "windows")]
const fn windows_drop_effect_payload(operation: FileClipboardOperation) -> u32 {
    operation.drop_effect()
}

#[cfg(not(target_os = "windows"))]
#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Non-Windows desktop command fallback.
pub fn copy_native(_paths: &[PathBuf]) -> Result<(), AppError> {
    set_macos_file_clipboard(_paths, FileClipboardOperation::Copy)
}

#[cfg(not(target_os = "windows"))]
#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // Non-Windows desktop command fallback.
pub fn move_native(paths: &[PathBuf]) -> Result<(), AppError> {
    set_macos_file_clipboard(paths, FileClipboardOperation::Move)
}

#[cfg(target_os = "macos")]
fn set_macos_file_clipboard(
    paths: &[PathBuf],
    _operation: FileClipboardOperation,
) -> Result<(), AppError> {
    // AppKit's public file-URL pasteboard model intentionally represents Copy
    // and Move identically. Finder performs the move only for Option-Command-V.
    macos_write_file_urls(paths)
}

#[cfg(not(target_os = "macos"))]
#[allow(dead_code)] // Windows has its own CF_HDROP implementation; this keeps the cross-platform command boundary uniform.
fn set_macos_file_clipboard(
    _paths: &[PathBuf],
    _operation: FileClipboardOperation,
) -> Result<(), AppError> {
    Err(AppError::DesktopActionFailed)
}

/// Public `file://` URL rendering used by the AppKit bridge and tests. It is
/// percent-encoded, never parsed as a shell string, and accepts only canonical
/// paths supplied by the durable-ID authorization functions above.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // macOS bridge uses this public-URL validation; Windows tests exercise encoding separately.
pub(crate) fn macos_file_url(path: &Path) -> Result<String, AppError> {
    let path = path.to_str().ok_or(AppError::DesktopActionFailed)?;
    let mut url = String::from("file://");
    for byte in path.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            url.push(*byte as char);
        } else {
            use std::fmt::Write;
            write!(&mut url, "%{byte:02X}").expect("string writes cannot fail");
        }
    }
    Ok(url)
}

#[cfg(target_os = "macos")]
fn macos_write_file_urls(paths: &[PathBuf]) -> Result<(), AppError> {
    use core::ffi::{c_char, c_void};
    // Narrow Objective-C bridge: only public NSPasteboard/NSURL APIs are
    // called. The URLs are passed as objects, never interpolated into a shell.
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    unsafe fn send_id(receiver: *mut c_void, selector: *mut c_void) -> *mut c_void {
        let call: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        call(receiver, selector)
    }
    unsafe fn send_id_id(
        receiver: *mut c_void,
        selector: *mut c_void,
        value: *mut c_void,
    ) -> *mut c_void {
        let call: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        call(receiver, selector, value)
    }
    unsafe fn send_bool_id(
        receiver: *mut c_void,
        selector: *mut c_void,
        value: *mut c_void,
    ) -> bool {
        let call: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> bool =
            std::mem::transmute(objc_msgSend as *const ());
        call(receiver, selector, value)
    }
    unsafe fn send_id_bytes_len_encoding(
        receiver: *mut c_void,
        selector: *mut c_void,
        bytes: *const u8,
        length: usize,
    ) -> *mut c_void {
        let call: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            *const u8,
            usize,
            usize,
        ) -> *mut c_void = std::mem::transmute(objc_msgSend as *const ());
        call(receiver, selector, bytes, length, 4) // NSUTF8StringEncoding
    }
    unsafe fn send_id_objects_count(
        receiver: *mut c_void,
        selector: *mut c_void,
        objects: *const *mut c_void,
        count: usize,
    ) -> *mut c_void {
        let call: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            *const *mut c_void,
            usize,
        ) -> *mut c_void = std::mem::transmute(objc_msgSend as *const ());
        call(receiver, selector, objects, count)
    }
    unsafe {
        let ns_string = objc_getClass(c"NSString".as_ptr());
        let ns_url = objc_getClass(c"NSURL".as_ptr());
        let ns_array = objc_getClass(c"NSArray".as_ptr());
        let pasteboard = objc_getClass(c"NSPasteboard".as_ptr());
        if ns_string.is_null() || ns_url.is_null() || ns_array.is_null() || pasteboard.is_null() {
            return Err(AppError::DesktopActionFailed);
        }
        let alloc = sel_registerName(c"alloc".as_ptr());
        let init_bytes = sel_registerName(c"initWithBytes:length:encoding:".as_ptr());
        let file_url = sel_registerName(c"fileURLWithPath:".as_ptr());
        let init_objects = sel_registerName(c"initWithObjects:count:".as_ptr());
        let general = sel_registerName(c"generalPasteboard".as_ptr());
        let clear = sel_registerName(c"clearContents".as_ptr());
        let write = sel_registerName(c"writeObjects:".as_ptr());
        let mut urls = Vec::with_capacity(paths.len());
        for path in paths {
            let _public_file_url = macos_file_url(path)?;
            let path = path.to_str().ok_or(AppError::DesktopActionFailed)?;
            let text = send_id_bytes_len_encoding(
                send_id(ns_string, alloc),
                init_bytes,
                path.as_ptr(),
                path.len(),
            );
            let url = send_id_id(ns_url, file_url, text);
            if url.is_null() {
                return Err(AppError::DesktopActionFailed);
            }
            urls.push(url);
        }
        let array = send_id_objects_count(
            send_id(ns_array, alloc),
            init_objects,
            urls.as_ptr(),
            urls.len(),
        );
        let board = send_id(pasteboard, general);
        if array.is_null() || board.is_null() {
            return Err(AppError::DesktopActionFailed);
        }
        let _ = send_id(board, clear);
        send_bool_id(board, write, array)
            .then_some(())
            .ok_or(AppError::DesktopActionFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{Batch, SettingsRepository, TransferItem};

    fn repository() -> (tempfile::TempDir, SettingsRepository) {
        let dir = tempfile::tempdir().unwrap();
        let repo = SettingsRepository::open(dir.path().join("state.sqlite")).unwrap();
        (dir, repo)
    }

    fn seed(repo: &SettingsRepository, path: &Path) {
        repo.save_batch(&Batch {
            id: "b".into(),
            direction: "incoming".into(),
            state: "completed".into(),
            created_at: 1,
            completed_at: Some(1),
            total_bytes: 1,
            total_entries: 1,
            warning_count: 0,
            wait_for_available: false,
        })
        .unwrap();
        repo.save_item(&TransferItem {
            id: "i".into(),
            batch_id: "b".into(),
            parent_item_id: None,
            kind: "file".into(),
            display_name: "safe.txt".into(),
            source_path_local: None,
            destination_path_local: Some(path.display().to_string()),
            size: 1,
            mtime: None,
            state: "completed".into(),
            warning_json: None,
        })
        .unwrap();
    }

    #[test]
    fn ids_authorize_only_existing_completed_incoming_outputs() {
        let (dir, repo) = repository();
        let path = dir.path().join("safe.txt");
        std::fs::write(&path, b"x").unwrap();
        seed(&repo, &path);
        assert_eq!(
            completed_output_for_item(&repo, "i").unwrap(),
            path.canonicalize().unwrap()
        );
        assert!(completed_output_for_item(&repo, "../outside").is_err());
        std::fs::remove_file(path).unwrap();
        assert!(completed_output_for_item(&repo, "i").is_err());
    }

    #[test]
    fn completed_batch_actions_resolve_all_and_only_authorized_top_level_outputs() {
        let (dir, repo) = repository();
        let first = dir.path().join("first.txt");
        let second = dir.path().join("second.txt");
        std::fs::write(&first, b"one").unwrap();
        std::fs::write(&second, b"two").unwrap();
        seed(&repo, &first);
        repo.save_item(&TransferItem {
            id: "second".into(),
            batch_id: "b".into(),
            parent_item_id: None,
            kind: "file".into(),
            display_name: "second.txt".into(),
            source_path_local: None,
            destination_path_local: Some(second.display().to_string()),
            size: 3,
            mtime: None,
            state: "completed".into(),
            warning_json: None,
        })
        .unwrap();
        assert_eq!(
            completed_outputs_for_batch(&repo, "b").unwrap(),
            vec![
                first.canonicalize().unwrap(),
                second.canonicalize().unwrap()
            ]
        );
        assert!(completed_outputs_for_batch(&repo, "not-a-batch").is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_cut_payload_requests_move_drop_effect() {
        assert_eq!(windows_drop_effect_payload(FileClipboardOperation::Copy), 1);
        assert_eq!(windows_drop_effect_payload(FileClipboardOperation::Move), 2);
    }

    #[test]
    fn macos_file_urls_are_percent_encoded_not_shell_interpolated() {
        let value = macos_file_url(Path::new("/tmp/space ; $(not-a-command).txt")).unwrap();
        assert_eq!(
            value,
            "file:///tmp/space%20%3B%20%24%28not-a-command%29.txt"
        );
        assert!(!value.contains(' '));
        assert!(!value.contains("$("));
    }

    #[test]
    fn notification_copy_is_redacted() {
        let (_, body) = incoming_notification_text(true, 2);
        assert_eq!(body, "Received 2 item(s).");
        assert!(!body.contains("C:\\"));
    }
}
