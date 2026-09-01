use std::sync::OnceLock;
use tauri::{
    menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder, Submenu, SubmenuBuilder},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, Wry,
};

pub const MENU_SHOW: &str = "show";
pub const MENU_RECEIVING: &str = "receiving";
pub const MENU_SETTINGS: &str = "settings";
pub const MENU_QUIT: &str = "quit";
const RECENT_ITEM_PREFIX: &str = "recent-item:";
static STATUS_ITEM: OnceLock<tauri::menu::MenuItem<Wry>> = OnceLock::new();
static RECEIVING_ITEM: OnceLock<tauri::menu::CheckMenuItem<Wry>> = OnceLock::new();
static RECENT_SUBMENU: OnceLock<Submenu<Wry>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecentReceivedAction {
    menu_id: String,
    label: String,
}

fn recent_received_model(
    history: &[crate::state::HistoryItemViewModel],
) -> Vec<RecentReceivedAction> {
    let mut batches = history
        .iter()
        .filter(|batch| batch.direction == "incoming" && batch.state == "completed")
        .collect::<Vec<_>>();
    batches.sort_by(|left, right| right.time_label.cmp(&left.time_label));
    batches
        .into_iter()
        .flat_map(|batch| batch.items.iter())
        .filter(|item| item.state == "completed" && item.available)
        .take(5)
        .map(|item| RecentReceivedAction {
            menu_id: format!("{RECENT_ITEM_PREFIX}{}", item.item_id),
            label: safe_recent_label(&item.display_name),
        })
        .collect()
}

fn safe_recent_label(label: &str) -> String {
    let value = label
        .chars()
        .filter(|character| !character.is_control())
        .take(80)
        .collect::<String>();
    let value = value.trim();
    if value.is_empty() {
        "Received item".into()
    } else {
        value.into()
    }
}

fn recent_item_id(menu_id: &str) -> Option<&str> {
    menu_id
        .strip_prefix(RECENT_ITEM_PREFIX)
        .filter(|id| !id.is_empty() && id.len() <= 128)
}

pub(crate) fn status_text(snapshot: &crate::state::AppSnapshot) -> String {
    format!(
        "{} device(s) connected; receiving {}",
        snapshot
            .devices
            .iter()
            .filter(|device| device.state == "online")
            .count(),
        if snapshot.lifecycle.receiving_enabled {
            "enabled"
        } else {
            "paused"
        }
    )
}
pub(crate) fn refresh(app: &AppHandle<Wry>, state: &crate::state::AppState) {
    let Ok(snapshot) = state.snapshot(false) else {
        return;
    };
    if let Some(status) = STATUS_ITEM.get() {
        let _ = status.set_text(status_text(&snapshot));
    }
    if let Some(receiving) = RECEIVING_ITEM.get() {
        let _ = receiving.set_checked(snapshot.lifecycle.receiving_enabled);
    }
    refresh_recent_submenu(app, &snapshot);
}

fn refresh_recent_submenu(app: &AppHandle<Wry>, snapshot: &crate::state::AppSnapshot) {
    let Some(submenu) = RECENT_SUBMENU.get() else {
        return;
    };
    for item in submenu.items().unwrap_or_default() {
        let _ = submenu.remove(&item);
    }
    let model = recent_received_model(&snapshot.history);
    let _ = submenu.set_enabled(!model.is_empty());
    if model.is_empty() {
        if let Ok(empty) = MenuItemBuilder::with_id("recent-empty", "No recent received items")
            .enabled(false)
            .build(app)
        {
            let _ = submenu.append(&empty);
        }
        return;
    }
    for action in model {
        if let Ok(item) = MenuItemBuilder::with_id(action.menu_id, action.label).build(app) {
            let _ = submenu.append(&item);
        }
    }
}

pub fn build(app: &AppHandle<Wry>) -> tauri::Result<()> {
    let status_text = app
        .try_state::<crate::state::AppState>()
        .and_then(|state| state.snapshot(false).ok())
        .map(|snapshot| status_text(&snapshot))
        .unwrap_or_else(|| "Receiving state unavailable".into());
    let status = MenuItemBuilder::with_id("status", status_text)
        .enabled(false)
        .build(app)?;
    let show = MenuItemBuilder::with_id(MENU_SHOW, "Show Fileporter").build(app)?;
    let receiving_enabled = app
        .try_state::<crate::state::AppState>()
        .and_then(|state| state.settings.load().ok())
        .map(|settings| settings.receiving_enabled)
        .unwrap_or(false);
    let receiving = CheckMenuItemBuilder::with_id(MENU_RECEIVING, "Receiving enabled")
        .checked(receiving_enabled)
        .build(app)?;
    let _ = STATUS_ITEM.set(status.clone());
    let _ = RECEIVING_ITEM.set(receiving.clone());
    let settings = MenuItemBuilder::with_id(MENU_SETTINGS, "Settings").build(app)?;
    let recent = SubmenuBuilder::with_id(app, "recent-received", "Recent received")
        .enabled(false)
        .build()?;
    let _ = RECENT_SUBMENU.set(recent.clone());
    if let Some(state) = app.try_state::<crate::state::AppState>() {
        if let Ok(snapshot) = state.snapshot(false) {
            refresh_recent_submenu(app, &snapshot);
        }
    }
    let quit = MenuItemBuilder::with_id(MENU_QUIT, "Quit Fileporter").build(app)?;
    let menu = MenuBuilder::new(app)
        .items(&[&status, &show, &receiving, &recent, &settings])
        .separator()
        .item(&quit)
        .build()?;
    TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .tooltip("Fileporter")
        .on_menu_event(move |app, event| match event.id.as_ref() {
            MENU_SHOW => {
                let _ = crate::app::show_main_window(app);
            }
            MENU_SETTINGS => {
                let _ = crate::app::show_main_window(app);
                let _ = app.emit("app://navigate", "settings");
            }
            MENU_RECEIVING => {
                let app = app.clone();
                let receiving = receiving.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) =
                        crate::commands::toggle_receiving_from_tray(app.clone()).await
                    {
                        tracing::warn!(
                            event_code = "tray.receiving_toggle_failed",
                            code = %error.code,
                            "could not change receiving state"
                        );
                    } else if let Some(state) = app.try_state::<crate::state::AppState>() {
                        if let Ok(settings) = state.settings.load() {
                            let _ = receiving.set_checked(settings.receiving_enabled);
                        }
                    }
                });
            }
            MENU_QUIT => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(state) = app.try_state::<crate::state::AppState>() {
                        state.shutdown().await;
                    }
                    app.exit(0);
                });
            }
            id => {
                if let Some(item_id) = recent_item_id(id) {
                    if let Some(state) = app.try_state::<crate::state::AppState>() {
                        if let Ok(path) = crate::desktop_actions::completed_output_for_item(
                            &state.settings,
                            item_id,
                        ) {
                            let _ = crate::desktop_actions::reveal_native(&[path]);
                        }
                    }
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                tauri::tray::TrayIconEvent::Click {
                    button: tauri::tray::MouseButton::Left,
                    button_state: tauri::tray::MouseButtonState::Up,
                    ..
                }
            ) {
                let _ = crate::app::show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{HistoryItemViewModel, HistoryTopLevelItemViewModel};
    fn batch(time: &str, items: Vec<(&str, &str, bool)>) -> HistoryItemViewModel {
        HistoryItemViewModel {
            id: time.into(),
            direction: "incoming".into(),
            peer_name: "peer".into(),
            summary: String::new(),
            time_label: time.into(),
            state: "completed".into(),
            items: items
                .into_iter()
                .map(|(id, name, available)| HistoryTopLevelItemViewModel {
                    item_id: id.into(),
                    display_name: name.into(),
                    kind: "file".into(),
                    size: 0,
                    state: "completed".into(),
                    available,
                    destination_label: None,
                })
                .collect(),
        }
    }
    #[test]
    fn recent_model_orders_truncates_and_uses_ids_only() {
        let model = recent_received_model(&[
            batch("10", vec![("old", "Old", true)]),
            batch(
                "20",
                vec![
                    ("new", "New", true),
                    ("two", "Two", true),
                    ("three", "Three", true),
                    ("four", "Four", true),
                    ("five", "Five", true),
                ],
            ),
        ]);
        assert_eq!(model.len(), 5);
        assert_eq!(model[0].menu_id, "recent-item:new");
        assert!(model
            .iter()
            .all(|item| !item.menu_id.contains('\\') && !item.menu_id.contains('/')));
    }
    #[test]
    fn recent_model_hides_unavailable_and_sanitizes_labels() {
        let model = recent_received_model(&[batch(
            "20",
            vec![("id", "\u{0} Report", true), ("gone", "Gone", false)],
        )]);
        assert_eq!(model[0].label, "Report");
        assert_eq!(recent_item_id(&model[0].menu_id), Some("id"));
        assert!(recent_received_model(&[]).is_empty());
        assert_eq!(recent_item_id("recent-item:"), None);
    }
}
