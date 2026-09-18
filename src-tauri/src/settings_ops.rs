//! Settings validation and persistence shared by every shell. Desktop commands
//! and the iOS bridge apply the same rules, so a setting accepted on one
//! platform means the same thing on the other.
use crate::{
    engine::validate_listen_address,
    error::{AppError, AppErrorDto},
    persistence::Settings,
};
use serde::Deserialize;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // The iOS bridge has its own onboarding input.
pub struct CompleteOnboardingInput {
    pub device_name: String,
    pub receive_directory: String,
    #[serde(default)]
    pub launch_at_login: Option<bool>,
    #[serde(default)]
    pub notifications_enabled: Option<bool>,
    #[serde(default)]
    pub automatic_device_trust: Option<bool>,
}
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettingsInput {
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub receive_directory: Option<String>,
    #[serde(default)]
    pub receiving_enabled: Option<bool>,
    #[serde(default)]
    pub listen_address: Option<String>,
    #[serde(default)]
    pub launch_at_login: Option<bool>,
    #[serde(default)]
    pub notifications_enabled: Option<bool>,
    #[serde(default)]
    pub automatic_device_trust: Option<bool>,
    #[serde(default)]
    pub history_retention_days: Option<i64>,
}
pub fn validate_device_name(name: &str) -> Result<String, AppErrorDto> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 48 {
        return Err(AppError::Validation {
            code: "invalid_device_name",
            message: "Name must be 1–48 characters",
            field: Some("deviceName"),
        }
        .into());
    }
    Ok(trimmed.to_owned())
}
pub fn probe_receive_directory(path: &Path) -> Result<PathBuf, AppErrorDto> {
    if path.as_os_str().is_empty() {
        return Err(AppError::Validation {
            code: "invalid_path",
            message: "Folder required",
            field: Some("receiveDirectory"),
        }
        .into());
    }
    fs::create_dir_all(path).map_err(|_| AppErrorDto::from(AppError::DestinationUnwritable))?;
    let canonical = path
        .canonicalize()
        .map_err(|_| AppErrorDto::from(AppError::DestinationUnwritable))?;
    if !canonical.is_dir() {
        return Err(AppError::Validation {
            code: "invalid_path",
            message: "Not a folder",
            field: Some("receiveDirectory"),
        }
        .into());
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let probe = canonical.join(format!(
        ".fileporter-write-probe-{}-{nonce}",
        std::process::id()
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)?;
        file.write_all(b"")?;
        file.sync_all()?;
        fs::remove_file(&probe)
    })();
    result.map_err(|_| AppErrorDto::from(AppError::DestinationUnwritable))?;
    Ok(canonical)
}
pub fn apply_settings_patch(
    settings: &mut Settings,
    input: UpdateSettingsInput,
) -> Result<(), AppErrorDto> {
    if let Some(name) = input.device_name {
        settings.device_name = validate_device_name(&name)?;
    }
    if let Some(directory) = input.receive_directory {
        settings.receive_directory = Some(
            probe_receive_directory(Path::new(&directory))?
                .display()
                .to_string(),
        );
    }
    if let Some(value) = input.receiving_enabled {
        settings.receiving_enabled = value;
    }
    if let Some(address) = input.listen_address {
        validate_listen_address(&address).map_err(|_| AppError::Validation {
            code: "invalid_listen_address",
            message: "Invalid listen address",
            field: Some("listenAddress"),
        })?;
        settings.listen_address = address;
    }
    if let Some(value) = input.launch_at_login {
        settings.launch_at_login = value;
    }
    if let Some(value) = input.notifications_enabled {
        settings.notifications_enabled = value;
    }
    if let Some(value) = input.automatic_device_trust {
        settings.automatic_device_trust = value;
    }
    if let Some(days) = input.history_retention_days {
        validate_history_retention(days)?;
        settings.history_retention_days = days;
    }
    Ok(())
}
fn validate_history_retention(days: i64) -> Result<(), AppErrorDto> {
    matches!(days, 0 | 7 | 30 | 90)
        .then_some(())
        .ok_or_else(|| {
            AppError::Validation {
                code: "invalid_history_retention",
                message: "Invalid retention",
                field: Some("historyRetentionDays"),
            }
            .into()
        })
}
#[cfg_attr(not(any(feature = "desktop", feature = "mobile")), allow(dead_code))] // Called by the desktop commands and the iOS bridge.
pub fn apply_history_retention(
    repository: &crate::persistence::SettingsRepository,
    settings: &Settings,
) -> Result<(), AppError> {
    if settings.history_retention_days > 0 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        repository.prune_terminal_history_before(
            now.saturating_sub(settings.history_retention_days * 86_400),
        )?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_name_is_trimmed_and_bounded() {
        assert_eq!(validate_device_name("  Work PC  ").unwrap(), "Work PC");
        assert!(validate_device_name("").is_err());
        assert!(validate_device_name(&"a".repeat(49)).is_err());
    }
    #[test]
    fn probe_creates_and_leaves_an_empty_folder_clean() {
        let parent = tempfile::tempdir().unwrap();
        let receive = parent.path().join("receive");
        let result = probe_receive_directory(&receive).unwrap();
        assert_eq!(result, receive.canonicalize().unwrap());
        assert_eq!(fs::read_dir(receive).unwrap().count(), 0);
    }
    #[test]
    fn settings_patch_matches_camel_case_input() {
        let patch: UpdateSettingsInput = serde_json::from_str(
            r#"{"deviceName":"Office PC","receivingEnabled":false,"automaticDeviceTrust":false}"#,
        )
        .unwrap();
        let mut settings = Settings::default();
        apply_settings_patch(&mut settings, patch).unwrap();
        assert_eq!(settings.device_name, "Office PC");
        assert!(!settings.receiving_enabled);
        assert!(!settings.automatic_device_trust);
    }
    #[test]
    fn settings_patch_accepts_only_documented_history_retention_values() {
        let patch: UpdateSettingsInput =
            serde_json::from_str(r#"{"historyRetentionDays":90}"#).unwrap();
        let mut settings = Settings::default();
        apply_settings_patch(&mut settings, patch).unwrap();
        assert_eq!(settings.history_retention_days, 90);
        let invalid: UpdateSettingsInput =
            serde_json::from_str(r#"{"historyRetentionDays":14}"#).unwrap();
        assert!(apply_settings_patch(&mut settings, invalid).is_err());
    }
    #[test]
    fn update_settings_camel_case_retention_patch_persists() {
        let directory = tempfile::tempdir().unwrap();
        let repository =
            crate::persistence::SettingsRepository::open(directory.path().join("settings.sqlite"))
                .unwrap();
        let patch: UpdateSettingsInput =
            serde_json::from_str(r#"{"historyRetentionDays":7}"#).unwrap();
        let mut settings = repository.load().unwrap();
        apply_settings_patch(&mut settings, patch).unwrap();
        repository.save(&settings).unwrap();
        assert_eq!(repository.load().unwrap().history_retention_days, 7);
    }
}
