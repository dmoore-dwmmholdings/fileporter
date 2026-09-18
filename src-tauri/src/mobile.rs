//! The iOS shell's boundary to the shared core.
//!
//! Swift drives the same `AppState` the desktop commands do through three C
//! functions: start once, call named commands with JSON input, and free the
//! JSON strings returned. Every reply is `{"ok": value}` or
//! `{"error": AppErrorDto}`, so Swift decodes one envelope for every command.
//! State changes are pushed as a bare revision through the start callback; the
//! app then asks for a fresh snapshot, exactly as the desktop webview does.

use std::{
    ffi::{c_char, c_void, CStr, CString},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    engine::validate_manual_endpoint,
    error::{AppError, AppErrorDto},
    persistence::{default_database_path, SettingsRepository},
    settings_ops::{
        apply_history_retention, apply_settings_patch, probe_receive_directory,
        validate_device_name, UpdateSettingsInput,
    },
    state::{AppState, EnqueuePathsRequest},
    state_events::StateEventWorker,
};

/// Called from a core thread with the snapshot revision that just advanced.
pub type ChangeCallback = extern "C" fn(context: *mut c_void, revision: u64);

struct Core {
    runtime: tokio::runtime::Runtime,
    state: AppState,
    /// The app container moves between installs, so the receive folder is
    /// supplied on every launch rather than trusted from persistence.
    receive_directory: PathBuf,
    notify: Notifier,
    active: AtomicBool,
}

#[derive(Clone, Copy)]
struct Notifier {
    callback: Option<ChangeCallback>,
    context: usize,
}

impl Notifier {
    fn changed(&self, state: &AppState) {
        state.bump_revision();
        if let Some(callback) = self.callback {
            callback(self.context as *mut c_void, state.revision());
        }
    }
}

static CORE: OnceLock<Result<Core, AppErrorDto>> = OnceLock::new();

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum Reply<T: Serialize> {
    Ok(T),
    Error(AppErrorDto),
}

fn reply<T: Serialize>(result: Result<T, AppErrorDto>) -> *mut c_char {
    let envelope = match result {
        Ok(value) => Reply::Ok(value),
        Err(error) => Reply::Error(error),
    };
    let json = serde_json::to_string(&envelope).unwrap_or_else(|_| {
        r#"{"error":{"code":"internal","message":"Reply failed","retryable":false}}"#.into()
    });
    CString::new(json)
        .unwrap_or_else(|_| CString::new("{}").expect("static"))
        .into_raw()
}

fn invalid(message: &'static str) -> AppErrorDto {
    AppError::Validation {
        code: "invalid_request",
        message,
        field: None,
    }
    .into()
}

fn read_str(pointer: *const c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: callers pass NUL-terminated UTF-8 strings valid for the call.
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .ok()
        .map(str::to_owned)
}

/// Opens the database, identity, listener, and scheduler. Idempotent: a second
/// call returns the first result.
///
/// # Safety
/// `data_directory` and `receive_directory` must be NUL-terminated strings.
/// `context` is passed back untouched to `callback` from any thread.
#[no_mangle]
pub unsafe extern "C" fn fileporter_start(
    data_directory: *const c_char,
    receive_directory: *const c_char,
    callback: Option<ChangeCallback>,
    context: *mut c_void,
) -> *mut c_char {
    let (Some(data), Some(receive)) = (read_str(data_directory), read_str(receive_directory))
    else {
        return reply::<()>(Err(invalid("Storage missing")));
    };
    let notify = Notifier {
        callback,
        context: context as usize,
    };
    let core = CORE.get_or_init(|| start(Path::new(&data), Path::new(&receive), notify));
    reply(core.as_ref().map(|_| ()).map_err(Clone::clone))
}

fn start(data: &Path, receive: &Path, notify: Notifier) -> Result<Core, AppErrorDto> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .thread_name("fileporter-core")
        .build()
        .map_err(|_| AppErrorDto::from(AppError::DataDirectoryUnavailable))?;
    std::fs::create_dir_all(data).map_err(|_| AppError::DataDirectoryUnavailable)?;
    crate::logging::initialize(&data.join("logs"));
    let receive_directory = probe_receive_directory(receive)?;
    let state = runtime.block_on(async {
        let repository = SettingsRepository::open(default_database_path(data))?;
        let mut settings = repository.load()?;
        let receive_label = receive_directory.display().to_string();
        if settings.receive_directory.as_deref() != Some(receive_label.as_str()) {
            settings.receive_directory = Some(receive_label);
            repository.save(&settings)?;
        }
        let (events, mut event_rx, worker) = StateEventWorker::bounded(16);
        let state = AppState::try_new_with_events(repository, events)?;
        state.attach_event_worker(worker);
        let event_state = state.clone();
        tokio::spawn(async move {
            while event_rx.recv().await.is_some() {
                notify.changed(&event_state);
            }
        });
        state.reconcile_listener().await?;
        state.start_sender_scheduler();
        Ok::<_, AppError>(state)
    })?;
    let core = Core {
        runtime,
        state,
        receive_directory,
        notify,
        active: AtomicBool::new(true),
    };
    // iOS reports no interface changes to the core. While the app is in the
    // foreground, the same idempotent reconcile the desktop polls keeps the
    // listener and advertisement honest after Wi-Fi changes.
    let poll_state = core.state.clone();
    core.runtime.spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let active = CORE
                .get()
                .and_then(|core| core.as_ref().ok())
                .map(|core| core.active.load(Ordering::Acquire))
                .unwrap_or(false);
            if active {
                let _ = poll_state.resume_or_network_changed().await;
            }
        }
    });
    Ok(core)
}

/// Runs one named command. Blocks the calling thread, so Swift calls it off
/// the main actor.
///
/// # Safety
/// `command` and `input` must be NUL-terminated strings; `input` may be null.
#[no_mangle]
pub unsafe extern "C" fn fileporter_call(
    command: *const c_char,
    input: *const c_char,
) -> *mut c_char {
    let Some(command) = read_str(command) else {
        return reply::<()>(Err(invalid("Unknown command")));
    };
    let input: Value = read_str(input)
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    let core = match CORE.get() {
        Some(Ok(core)) => core,
        Some(Err(error)) => return reply::<()>(Err(error.clone())),
        None => return reply::<()>(Err(AppError::DataDirectoryUnavailable.into())),
    };
    reply(dispatch(core, &command, input))
}

/// Frees a string returned by this module.
///
/// # Safety
/// `value` must come from `fileporter_start` or `fileporter_call`, once.
#[no_mangle]
pub unsafe extern "C" fn fileporter_free_string(value: *mut c_char) {
    if !value.is_null() {
        // SAFETY: the pointer was produced by `CString::into_raw` above.
        drop(unsafe { CString::from_raw(value) });
    }
}

fn parse<T: for<'de> Deserialize<'de>>(input: Value) -> Result<T, AppErrorDto> {
    serde_json::from_value(input).map_err(|_| invalid("Invalid request"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnboardingInput {
    device_name: String,
    #[serde(default)]
    notifications_enabled: Option<bool>,
    #[serde(default)]
    automatic_device_trust: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdInput {
    #[serde(
        alias = "batchId",
        alias = "itemId",
        alias = "pairingId",
        alias = "deviceId"
    )]
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameInput {
    device_id: String,
    alias: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EndpointInput {
    endpoint: String,
}

fn to_value<T: Serialize>(value: T) -> Result<Value, AppErrorDto> {
    serde_json::to_value(value).map_err(|_| AppError::EventEmissionFailed.into())
}

fn dispatch(core: &Core, command: &str, input: Value) -> Result<Value, AppErrorDto> {
    // Swift calls in from its own dispatch threads. Scheduling and pairing
    // spawn Tokio tasks, which panic outside a runtime context.
    let _runtime = core.runtime.enter();
    let state = &core.state;
    let changed = || core.notify.changed(state);
    match command {
        "snapshot" => to_value(state.snapshot(core.active.load(Ordering::Acquire))?),
        "completeOnboarding" => {
            let input: OnboardingInput = parse(input)?;
            let previous = state.settings.load()?;
            let mut settings = previous.clone();
            settings.device_name = validate_device_name(&input.device_name)?;
            settings.receive_directory = Some(core.receive_directory.display().to_string());
            settings.onboarding_complete = true;
            // There is no login item on iOS.
            settings.launch_at_login = false;
            if let Some(value) = input.notifications_enabled {
                settings.notifications_enabled = value;
            }
            if let Some(value) = input.automatic_device_trust {
                settings.automatic_device_trust = value;
            }
            state.settings.save(&settings)?;
            let applied = apply_history_retention(&state.settings, &settings)
                .map_err(AppErrorDto::from)
                .and_then(|()| {
                    core.runtime
                        .block_on(state.reconcile_listener())
                        .map_err(AppErrorDto::from)
                });
            if let Err(error) = applied {
                let _ = state.settings.save(&previous);
                return Err(error);
            }
            state.start_sender_scheduler();
            changed();
            to_value(state.snapshot(true)?)
        }
        "updateSettings" => {
            let mut patch: UpdateSettingsInput = parse(input)?;
            // The receive folder is the app's own; login items do not exist.
            patch.receive_directory = None;
            patch.launch_at_login = None;
            let previous = state.settings.load()?;
            let mut settings = previous.clone();
            apply_settings_patch(&mut settings, patch)?;
            state.settings.save(&settings)?;
            let applied = apply_history_retention(&state.settings, &settings)
                .map_err(AppErrorDto::from)
                .and_then(|()| {
                    core.runtime
                        .block_on(state.reconcile_listener())
                        .map_err(AppErrorDto::from)
                });
            if let Err(error) = applied {
                let _ = state.settings.save(&previous);
                return Err(error);
            }
            changed();
            to_value(state.snapshot(true)?)
        }
        "enqueuePaths" => {
            let request: EnqueuePathsRequest = parse(input)?;
            let batch = state.queue_batch(request)?;
            state.start_sender_scheduler();
            changed();
            to_value(batch)
        }
        "cancelBatch" => {
            let IdInput { id } = parse(input)?;
            state.cancel_batch(&id)?;
            changed();
            to_value(state.snapshot(true)?)
        }
        "retryBatch" => {
            let IdInput { id } = parse(input)?;
            state.retry_batch(&id)?;
            state.start_sender_scheduler();
            changed();
            to_value(state.snapshot(true)?)
        }
        "startPairingAtEndpoint" => {
            let EndpointInput { endpoint } = parse(input)?;
            let endpoint =
                validate_manual_endpoint(&endpoint).map_err(|_| AppError::Validation {
                    code: "invalid_pairing",
                    message: "Invalid pairing",
                    field: Some("endpoint"),
                })?;
            let name = state.settings.load()?.device_name;
            let pairing = core
                .runtime
                .block_on(state.engine.start_pairing_at_endpoint(endpoint, name))
                .map_err(|_| AppError::ListenerUnavailable)?;
            changed();
            to_value(pairing)
        }
        "startPairingDiscovered" => {
            let IdInput { id } = parse(input)?;
            let pairing = core.runtime.block_on(state.start_pairing_discovered(&id))?;
            changed();
            to_value(pairing)
        }
        "renameTrustedDevice" => {
            let RenameInput { device_id, alias } = parse(input)?;
            state.rename_trusted_device(&device_id, &alias)?;
            changed();
            to_value(())
        }
        "confirmPairing" => {
            let IdInput { id } = parse(input)?;
            let peer = state.engine.confirm_pairing(&id)?;
            changed();
            to_value(peer)
        }
        "rejectPairing" => {
            let IdInput { id } = parse(input)?;
            state.engine.reject_pairing(&id)?;
            changed();
            to_value(())
        }
        // Resolves durable ids to files the app may share or preview. Only
        // completed incoming outputs that still exist are ever returned.
        "itemPaths" => {
            let IdInput { id } = parse(input)?;
            let path = crate::desktop_actions::completed_output_for_item(&state.settings, &id)?;
            to_value(vec![path.display().to_string()])
        }
        "batchPaths" => {
            let IdInput { id } = parse(input)?;
            let paths = crate::desktop_actions::completed_outputs_for_batch(&state.settings, &id)?;
            to_value(
                paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>(),
            )
        }
        // iOS suspends a backgrounded app's sockets. Stop cleanly at a durable
        // checkpoint, then rebind and re-advertise on return.
        "suspend" => {
            core.active.store(false, Ordering::Release);
            core.runtime.block_on(state.suspend());
            changed();
            to_value(())
        }
        "resume" => {
            core.active.store(true, Ordering::Release);
            core.runtime.block_on(state.resume_or_network_changed())?;
            changed();
            to_value(state.snapshot(true)?)
        }
        _ => Err(invalid("Unknown command")),
    }
}
