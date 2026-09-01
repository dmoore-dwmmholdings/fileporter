use tauri::{Manager, RunEvent, WebviewWindow};

use crate::{
    commands,
    error::AppError,
    logging,
    persistence::{default_database_path, SettingsRepository},
    state::AppState,
    state_events::{StateEventKind, StateEventWorker},
};

pub fn run() {
    let background_launch = std::env::args().any(|argument| argument == "--background");
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let _ = show_main_window(app);
        }))
        .invoke_handler(tauri::generate_handler![
            commands::get_app_snapshot,
            commands::choose_files,
            commands::choose_directory,
            commands::choose_receive_directory,
            commands::view_logs,
            commands::export_logs,
            commands::complete_onboarding,
            commands::update_settings,
            commands::enqueue_paths,
            commands::send_queued_loopback,
            commands::cancel_batch,
            commands::retry_batch,
            commands::reveal_item,
            commands::copy_item,
            commands::move_item,
            commands::reveal_completed_batch,
            commands::copy_completed_batch,
            commands::move_completed_batch,
            commands::request_pairing,
            commands::start_pairing_at_endpoint,
            commands::start_pairing_discovered,
            commands::rename_trusted_device,
            commands::confirm_pairing,
            commands::reject_pairing,
            commands::forget_device,
            commands::show_main_window,
            commands::quit_app
        ])
        .setup(move |app| {
            let app_data = app
                .path()
                .app_data_dir()
                .map_err(|_| AppError::DataDirectoryUnavailable)?;
            logging::initialize(&app_data.join("logs"));
            let repository = SettingsRepository::open(default_database_path(&app_data))?;
            let (events, mut event_rx, event_worker) = StateEventWorker::bounded(16);
            let state = AppState::try_new_with_events(repository, events)?;
            state.attach_event_worker(event_worker);
            let settings = state.settings.load()?;
            let onboarding_complete = settings.onboarding_complete;
            tauri::async_runtime::block_on(state.reconcile_listener())?;
            if onboarding_complete {
                state.start_sender_scheduler();
            }
            app.manage(state);
            let event_state = app.state::<AppState>().inner().clone();
            let event_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                while let Some(kind) = event_rx.recv().await {
                    // All event producers persist before signalling. This is the
                    // only desktop adapter: it reads that durable snapshot.
                    let _ = commands::emit_snapshot(&event_app, &event_state);
                    crate::tray::refresh(&event_app, &event_state);
                    if matches!(kind, StateEventKind::Terminal) {
                        tracing::debug!(event = "terminal", "durable state event delivered");
                    }
                }
            });
            // Tauri exposes resume but no portable interface-change event;
            // bounded polling reuses the idempotent reconcile path.
            let network_state = app.state::<AppState>().inner().clone();
            tauri::async_runtime::spawn(async move {
                let mut detector = crate::lifecycle_monitor::WakeGapDetector::new(
                    std::time::Instant::now(),
                    std::time::Duration::from_secs(30),
                );
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    if matches!(
                        detector.tick(std::time::Instant::now()),
                        crate::lifecycle_monitor::WakeAction::SuspendThenResume
                    ) {
                        network_state.suspend().await;
                    }
                    let _ = network_state.resume_or_network_changed().await;
                }
            });
            crate::tray::build(app.handle())?;

            if onboarding_complete && settings.receiving_enabled {
                tracing::info!(
                    event_code = "listener.reconciled",
                    "receiving listener reconciled from persisted settings"
                );
            }
            if !background_launch || !onboarding_complete {
                show_main_window(app.handle())?;
            }
            Ok(())
        });

    builder
        .build(tauri::generate_context!())
        .expect("error while building Fileporter")
        .run(|app, event| {
            if let RunEvent::WindowEvent {
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } = event
            {
                api.prevent_close();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
                tracing::info!(
                    event_code = "window.hidden_to_tray",
                    "main window hidden; background process remains active"
                );
            } else if matches!(event, RunEvent::Resumed) {
                if let Some(state) = app.try_state::<AppState>() {
                    let state = state.inner().clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = state.resume_or_network_changed().await;
                    });
                }
            } else if let RunEvent::ExitRequested { api, .. } = event {
                if let Some(state) = app.try_state::<AppState>() {
                    if !state.shutdown_complete() {
                        api.prevent_exit();
                        let state = state.inner().clone();
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            state.shutdown().await;
                            app.exit(0);
                        });
                    }
                }
            }
        });
}

pub fn show_main_window(app: &tauri::AppHandle) -> Result<(), AppError> {
    let window: WebviewWindow = app
        .get_webview_window("main")
        .ok_or(AppError::MainWindowUnavailable)?;
    window.show().map_err(|_| AppError::MainWindowUnavailable)?;
    window
        .unminimize()
        .map_err(|_| AppError::MainWindowUnavailable)?;
    window
        .set_focus()
        .map_err(|_| AppError::MainWindowUnavailable)?;
    Ok(())
}
