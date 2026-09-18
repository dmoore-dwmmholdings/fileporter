use crate::identity::{PairingConfirmationView, PendingPairingView};
use crate::settings_ops::{
    apply_history_retention, apply_settings_patch, probe_receive_directory, validate_device_name,
    CompleteOnboardingInput, UpdateSettingsInput,
};
use crate::{
    engine::validate_manual_endpoint,
    error::{AppError, AppErrorDto},
    state::{AppSnapshot, AppState, EnqueuePathsRequest, QueuedBatchDto},
};
use serde::Deserialize;
use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;
use tokio::sync::oneshot;

trait LaunchAtLoginAdapter {
    fn set_enabled(&self, enabled: bool) -> Result<(), AppError>;
}
struct TauriLaunchAtLogin(AppHandle);
impl LaunchAtLoginAdapter for TauriLaunchAtLogin {
    fn set_enabled(&self, enabled: bool) -> Result<(), AppError> {
        let result = if enabled {
            self.0.autolaunch().enable()
        } else {
            self.0.autolaunch().disable()
        };
        result.map_err(|_| AppError::DesktopActionFailed)
    }
}

fn apply_launch_at_login<A: LaunchAtLoginAdapter>(
    adapter: &A,
    enabled: bool,
) -> Result<(), AppError> {
    adapter.set_enabled(enabled)
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RequestPairingInput {
    pub remote_name: String,
    pub public_key: Vec<u8>,
    pub certificate_fingerprint: String,
}
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairingIdInput {
    pub pairing_id: String,
}
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StartPairingAtEndpointInput {
    pub endpoint: String,
}
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredDeviceInput {
    pub device_id: String,
}
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RenameTrustedDeviceInput {
    pub device_id: String,
    pub alias: String,
}
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SendQueuedLoopbackInput {
    pub batch_id: String,
}
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BatchIdInput {
    pub batch_id: String,
}
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ItemIdInput {
    pub item_id: String,
}

#[tauri::command]
pub fn get_app_snapshot(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<AppSnapshot, AppErrorDto> {
    let visible = app
        .get_webview_window("main")
        .map(|window| window.is_visible().unwrap_or(false))
        .unwrap_or(false);
    state.snapshot(visible).map_err(Into::into)
}
/// The dialog plugin dispatches the native picker onto the main thread and
/// reports the choice through a callback. Awaiting that callback keeps these
/// commands off the main thread: a blocking pick issued from a synchronous
/// command would block the very event loop that has to run the picker.
async fn pick_files(app: &AppHandle) -> Option<Vec<tauri_plugin_dialog::FilePath>> {
    let (tx, rx) = oneshot::channel();
    app.dialog().file().pick_files(move |paths| {
        let _ = tx.send(paths);
    });
    rx.await.ok().flatten()
}
async fn pick_folder(app: &AppHandle) -> Option<tauri_plugin_dialog::FilePath> {
    let (tx, rx) = oneshot::channel();
    app.dialog().file().pick_folder(move |path| {
        let _ = tx.send(path);
    });
    rx.await.ok().flatten()
}
#[tauri::command]
pub async fn choose_files(app: AppHandle) -> Result<Vec<String>, AppErrorDto> {
    pick_files(&app)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(dialog_path_to_string)
        .collect()
}
#[tauri::command]
pub async fn choose_directory(app: AppHandle) -> Result<Vec<String>, AppErrorDto> {
    pick_folder(&app)
        .await
        .map(dialog_path_to_string)
        .transpose()
        .map(|path| path.into_iter().collect())
}
#[tauri::command]
pub async fn choose_receive_directory(app: AppHandle) -> Result<Option<String>, AppErrorDto> {
    pick_folder(&app)
        .await
        .map(dialog_path_to_string)
        .transpose()
}
/// Opens only Fileporter's own canonical log directory. No caller-controlled
/// filesystem path crosses the command boundary.
#[tauri::command]
pub fn view_logs(app: AppHandle) -> Result<(), AppErrorDto> {
    let logs = app
        .path()
        .app_data_dir()
        .map_err(|_| AppErrorDto::from(AppError::DataDirectoryUnavailable))?
        .join("logs");
    let logs = logs
        .canonicalize()
        .map_err(|_| AppErrorDto::from(AppError::CompletedOutputUnavailable))?;
    crate::desktop_actions::reveal_native(&[logs]).map_err(Into::into)
}
/// Exports only Fileporter-owned log files to a folder chosen in the native
/// dialog. The webview cannot provide either a source or destination path.
#[tauri::command]
pub async fn export_logs(app: AppHandle) -> Result<Option<String>, AppErrorDto> {
    let Some(destination) = pick_folder(&app).await else {
        return Ok(None);
    };
    let destination = dialog_path_to_string(destination)?;
    let source = app
        .path()
        .app_data_dir()
        .map_err(|_| AppErrorDto::from(AppError::DataDirectoryUnavailable))?
        .join("logs")
        .canonicalize()
        .map_err(|_| AppErrorDto::from(AppError::CompletedOutputUnavailable))?;
    let export = Path::new(&destination).join(format!(
        "Fileporter logs {}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    ));
    fs::create_dir(&export).map_err(|_| AppErrorDto::from(AppError::DesktopActionFailed))?;
    for entry in
        fs::read_dir(&source).map_err(|_| AppErrorDto::from(AppError::DesktopActionFailed))?
    {
        let entry = entry.map_err(|_| AppErrorDto::from(AppError::DesktopActionFailed))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
            && name.starts_with("fileporter")
            && name.contains(".log")
        {
            fs::copy(entry.path(), export.join(name.as_ref()))
                .map_err(|_| AppErrorDto::from(AppError::DesktopActionFailed))?;
        }
    }
    Ok(Some(export.display().to_string()))
}

#[tauri::command]
pub async fn complete_onboarding(
    app: AppHandle,
    input: CompleteOnboardingInput,
    state: tauri::State<'_, AppState>,
) -> Result<AppSnapshot, AppErrorDto> {
    let previous = state.settings.load().map_err(AppErrorDto::from)?;
    let mut settings = previous.clone();
    settings.device_name = validate_device_name(&input.device_name)?;
    settings.receive_directory = Some(
        probe_receive_directory(Path::new(&input.receive_directory))?
            .display()
            .to_string(),
    );
    settings.onboarding_complete = true;
    if let Some(value) = input.launch_at_login {
        settings.launch_at_login = value;
    }
    if let Some(value) = input.notifications_enabled {
        settings.notifications_enabled = value;
    }
    if let Some(value) = input.automatic_device_trust {
        settings.automatic_device_trust = value;
    }
    let autostart = TauriLaunchAtLogin(app.clone());
    apply_launch_at_login(&autostart, settings.launch_at_login).map_err(AppErrorDto::from)?;
    state.settings.save(&settings).map_err(AppErrorDto::from)?;
    if let Err(error) = apply_history_retention(&state.settings, &settings) {
        let _ = apply_launch_at_login(&autostart, previous.launch_at_login);
        let _ = state.settings.save(&previous);
        return Err(error.into());
    }
    if let Err(error) = state.reconcile_listener().await {
        let _ = apply_launch_at_login(&autostart, previous.launch_at_login);
        let _ = state.settings.save(&previous);
        return Err(error.into());
    }
    state.start_sender_scheduler();
    state.bump_revision();
    emit_snapshot(&app, &state)
}
#[tauri::command]
pub async fn update_settings(
    app: AppHandle,
    input: UpdateSettingsInput,
    state: tauri::State<'_, AppState>,
) -> Result<AppSnapshot, AppErrorDto> {
    let previous = state.settings.load().map_err(AppErrorDto::from)?;
    let mut settings = previous.clone();
    apply_settings_patch(&mut settings, input)?;
    let autostart = TauriLaunchAtLogin(app.clone());
    if settings.launch_at_login != previous.launch_at_login {
        apply_launch_at_login(&autostart, settings.launch_at_login).map_err(AppErrorDto::from)?;
    }
    state.settings.save(&settings).map_err(AppErrorDto::from)?;
    if let Err(error) = apply_history_retention(&state.settings, &settings) {
        if settings.launch_at_login != previous.launch_at_login {
            let _ = apply_launch_at_login(&autostart, previous.launch_at_login);
        }
        let _ = state.settings.save(&previous);
        return Err(error.into());
    }
    if let Err(error) = state.reconcile_listener().await {
        if settings.launch_at_login != previous.launch_at_login {
            let _ = apply_launch_at_login(&autostart, previous.launch_at_login);
        }
        let _ = state.settings.save(&previous);
        return Err(error.into());
    }
    state.bump_revision();
    emit_snapshot(&app, &state)
}
#[tauri::command]
pub fn reveal_item(
    input: ItemIdInput,
    state: tauri::State<'_, AppState>,
) -> Result<(), AppErrorDto> {
    let path = crate::desktop_actions::completed_output_for_item(&state.settings, &input.item_id)?;
    crate::desktop_actions::reveal_native(&[path]).map_err(Into::into)
}
#[tauri::command]
pub fn copy_item(input: ItemIdInput, state: tauri::State<'_, AppState>) -> Result<(), AppErrorDto> {
    let path = crate::desktop_actions::completed_output_for_item(&state.settings, &input.item_id)?;
    crate::desktop_actions::copy_native(&[path]).map_err(Into::into)
}
/// Windows publishes a Cut payload; macOS publishes public file URLs and the
/// UI can teach Finder's Option-Command-V move gesture. Neither platform ever
/// deletes an output in response to this command.
#[tauri::command]
pub fn move_item(input: ItemIdInput, state: tauri::State<'_, AppState>) -> Result<(), AppErrorDto> {
    let path = crate::desktop_actions::completed_output_for_item(&state.settings, &input.item_id)?;
    crate::desktop_actions::move_native(&[path]).map_err(Into::into)
}
#[tauri::command]
pub fn reveal_completed_batch(
    input: BatchIdInput,
    state: tauri::State<'_, AppState>,
) -> Result<(), AppErrorDto> {
    let paths =
        crate::desktop_actions::completed_outputs_for_batch(&state.settings, &input.batch_id)?;
    crate::desktop_actions::reveal_native(&paths).map_err(Into::into)
}
#[tauri::command]
pub fn copy_completed_batch(
    input: BatchIdInput,
    state: tauri::State<'_, AppState>,
) -> Result<(), AppErrorDto> {
    let paths =
        crate::desktop_actions::completed_outputs_for_batch(&state.settings, &input.batch_id)?;
    crate::desktop_actions::copy_native(&paths).map_err(Into::into)
}
#[tauri::command]
pub fn move_completed_batch(
    input: BatchIdInput,
    state: tauri::State<'_, AppState>,
) -> Result<(), AppErrorDto> {
    let paths =
        crate::desktop_actions::completed_outputs_for_batch(&state.settings, &input.batch_id)?;
    crate::desktop_actions::move_native(&paths).map_err(Into::into)
}
#[tauri::command]
pub fn enqueue_paths(
    app: AppHandle,
    input: EnqueuePathsRequest,
    state: tauri::State<'_, AppState>,
) -> Result<QueuedBatchDto, AppErrorDto> {
    let batch = state.queue_batch(input).map_err(AppErrorDto::from)?;
    state.start_sender_scheduler();
    let snapshot = state.snapshot(true).map_err(AppErrorDto::from)?;
    app.emit("app://snapshot-changed", snapshot)
        .map_err(|_| AppErrorDto::from(AppError::EventEmissionFailed))?;
    Ok(batch)
}
#[tauri::command]
pub fn cancel_batch(
    app: AppHandle,
    input: BatchIdInput,
    state: tauri::State<'_, AppState>,
) -> Result<AppSnapshot, AppErrorDto> {
    state
        .cancel_batch(&input.batch_id)
        .map_err(AppErrorDto::from)?;
    emit_snapshot(&app, &state)
}
#[tauri::command]
pub fn retry_batch(
    app: AppHandle,
    input: BatchIdInput,
    state: tauri::State<'_, AppState>,
) -> Result<AppSnapshot, AppErrorDto> {
    state
        .retry_batch(&input.batch_id)
        .map_err(AppErrorDto::from)?;
    state.start_sender_scheduler();
    emit_snapshot(&app, &state)
}
/// Narrow direct-transfer API. It never discovers peers: the queued target must
/// already be trusted and carries its own persisted private-network endpoint.
#[tauri::command]
pub async fn send_queued_loopback(
    app: AppHandle,
    input: SendQueuedLoopbackInput,
    state: tauri::State<'_, AppState>,
) -> Result<AppSnapshot, AppErrorDto> {
    state
        .send_queued_file(&input.batch_id, tokio_util::sync::CancellationToken::new())
        .await
        .map_err(AppErrorDto::from)?;
    emit_snapshot(&app, &state)
}
#[tauri::command]
pub fn request_pairing(
    app: AppHandle,
    input: RequestPairingInput,
    state: tauri::State<'_, AppState>,
) -> Result<PendingPairingView, AppErrorDto> {
    let pairing = state
        .pairing
        .request(
            input.remote_name,
            input.public_key,
            input.certificate_fingerprint,
        )
        .map_err(AppErrorDto::from)?;
    state.bump_revision();
    let _ = emit_snapshot(&app, &state)?;
    Ok(pairing)
}
/// Explicit manual pairing entry point. It authenticates the endpoint and
/// leaves a pending record; it never trusts a peer merely for connecting.
#[tauri::command]
pub async fn start_pairing_at_endpoint(
    app: AppHandle,
    input: StartPairingAtEndpointInput,
    state: tauri::State<'_, AppState>,
) -> Result<PendingPairingView, AppErrorDto> {
    let endpoint = validate_manual_endpoint(&input.endpoint).map_err(|_| AppError::Validation {
        code: "invalid_pairing",
        message: "Invalid pairing",
        field: Some("endpoint"),
    })?;
    let name = state
        .settings
        .load()
        .map_err(AppErrorDto::from)?
        .device_name;
    let pairing = state
        .engine
        .start_pairing_at_endpoint(endpoint, name)
        .await
        .map_err(|_| AppError::ListenerUnavailable)?;
    state.bump_revision();
    let _ = emit_snapshot(&app, &state)?;
    Ok(pairing)
}
#[tauri::command]
pub async fn start_pairing_discovered(
    app: AppHandle,
    input: DiscoveredDeviceInput,
    state: tauri::State<'_, AppState>,
) -> Result<PendingPairingView, AppErrorDto> {
    let pairing = state
        .start_pairing_discovered(&input.device_id)
        .await
        .map_err(AppErrorDto::from)?;
    state.bump_revision();
    let _ = emit_snapshot(&app, &state)?;
    Ok(pairing)
}
#[tauri::command]
pub fn rename_trusted_device(
    app: AppHandle,
    input: RenameTrustedDeviceInput,
    state: tauri::State<'_, AppState>,
) -> Result<(), AppErrorDto> {
    state
        .rename_trusted_device(&input.device_id, &input.alias)
        .map_err(AppErrorDto::from)?;
    state.bump_revision();
    let _ = emit_snapshot(&app, &state)?;
    Ok(())
}
#[tauri::command]
pub fn confirm_pairing(
    app: AppHandle,
    input: PairingIdInput,
    state: tauri::State<'_, AppState>,
) -> Result<PairingConfirmationView, AppErrorDto> {
    let peer = state
        .engine
        .confirm_pairing(&input.pairing_id)
        .map_err(AppErrorDto::from)?;
    state.bump_revision();
    let _ = emit_snapshot(&app, &state)?;
    Ok(peer)
}
#[tauri::command]
pub fn reject_pairing(
    app: AppHandle,
    input: PairingIdInput,
    state: tauri::State<'_, AppState>,
) -> Result<(), AppErrorDto> {
    state
        .engine
        .reject_pairing(&input.pairing_id)
        .map_err(AppErrorDto::from)?;
    state.bump_revision();
    let _ = emit_snapshot(&app, &state)?;
    Ok(())
}
pub(crate) fn emit_snapshot(app: &AppHandle, state: &AppState) -> Result<AppSnapshot, AppErrorDto> {
    // Notification failure must not alter a completed transfer. The persistent
    // ledger prevents history from being replayed on later snapshots/restarts.
    if let Err(error) = crate::desktop_notifications::dispatch_terminal_incoming(
        &state.settings,
        &TauriNotifier(app.clone()),
        unix_timestamp(),
    ) {
        tracing::warn!(event_code = "notification.dispatch_failed", error = %error, "desktop notification was not delivered");
    }
    let snapshot = state.snapshot(true).map_err(AppErrorDto::from)?;
    app.emit("app://snapshot-changed", snapshot.clone())
        .map_err(|_| AppErrorDto::from(AppError::EventEmissionFailed))?;
    Ok(snapshot)
}

struct TauriNotifier(AppHandle);
impl crate::desktop_notifications::Notifier for TauriNotifier {
    fn send(&self, title: &str, body: &str) -> Result<(), AppError> {
        self.0
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|_| AppError::DesktopActionFailed)
    }
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Tray-only receiving toggle. It persists the preference before reconciling
/// the listener, then emits the truthful runtime snapshot.
pub async fn toggle_receiving_from_tray(app: AppHandle) -> Result<(), AppErrorDto> {
    let state = app.state::<AppState>();
    let mut settings = state.settings.load().map_err(AppErrorDto::from)?;
    settings.receiving_enabled = !settings.receiving_enabled;
    state.settings.save(&settings).map_err(AppErrorDto::from)?;
    state
        .reconcile_listener()
        .await
        .map_err(AppErrorDto::from)?;
    state.bump_revision();
    let _ = emit_snapshot(&app, &state)?;
    Ok(())
}
#[tauri::command]
pub fn show_main_window(app: AppHandle) -> Result<(), AppErrorDto> {
    crate::app::show_main_window(&app).map_err(Into::into)
}
#[tauri::command]
pub async fn quit_app(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), AppErrorDto> {
    state.shutdown().await;
    tracing::info!(
        event_code = "app.quit_requested",
        "explicit application quit requested"
    );
    app.exit(0);
    Ok(())
}

fn dialog_path_to_string(path: tauri_plugin_dialog::FilePath) -> Result<String, AppErrorDto> {
    path.into_path()
        .map(|path| path.display().to_string())
        .map_err(|_| {
            AppError::Validation {
                code: "invalid_path",
                message: "The selected location is not a local file path.",
                field: Some("paths"),
            }
            .into()
        })
}
