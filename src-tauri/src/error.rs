use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // DTO is the desktop command boundary; core tests exercise the domain error instead.
pub struct AppErrorDto {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}
#[derive(Debug, Error)]
pub enum AppError {
    #[error("database setup failed")]
    Database(#[source] rusqlite::Error),
    #[error("application data directory is unavailable")]
    DataDirectoryUnavailable,
    #[error("main window is unavailable")]
    MainWindowUnavailable,
    #[error("application event could not be delivered")]
    EventEmissionFailed,
    #[error("invalid input: {code}")]
    Validation {
        code: &'static str,
        message: &'static str,
        field: Option<&'static str>,
    },
    #[error("receive directory is not writable")]
    DestinationUnwritable,
    #[error("local identity storage is invalid")]
    IdentityStorageInvalid,
    #[error("listener lifecycle failed")]
    ListenerUnavailable,
    #[error("a completed local output is unavailable")]
    CompletedOutputUnavailable,
    #[error("native desktop action failed")]
    DesktopActionFailed,
    #[error("native clipboard is busy")]
    ClipboardBusy,
}
#[cfg_attr(not(feature = "desktop"), allow(dead_code))] // See AppErrorDto: conversion is only invoked by Tauri commands.
impl From<AppError> for AppErrorDto {
    fn from(value: AppError) -> Self {
        match value {
            AppError::Database(_) => Self {
                code: "internal".into(),
                message: "Settings unavailable".into(),
                retryable: true,
                field: None,
            },
            AppError::DataDirectoryUnavailable => Self {
                code: "internal".into(),
                message: "No data directory".into(),
                retryable: false,
                field: None,
            },
            AppError::MainWindowUnavailable => Self {
                code: "internal".into(),
                message: "No window".into(),
                retryable: true,
                field: None,
            },
            AppError::EventEmissionFailed => Self {
                code: "internal".into(),
                message: "Window failed".into(),
                retryable: true,
                field: None,
            },
            AppError::Validation {
                code,
                message,
                field,
            } => Self {
                code: code.into(),
                message: message.into(),
                retryable: false,
                field: field.map(str::to_owned),
            },
            AppError::DestinationUnwritable => Self {
                code: "destination_unwritable".into(),
                message: "Folder not writable".into(),
                retryable: true,
                field: Some("receiveDirectory".into()),
            },
            AppError::IdentityStorageInvalid => Self {
                code: "identity_storage_invalid".into(),
                message: "Identity unavailable".into(),
                retryable: false,
                field: None,
            },
            AppError::ListenerUnavailable => Self {
                code: "listener_unavailable".into(),
                message: "Listener failed".into(),
                retryable: true,
                field: Some("listenAddress".into()),
            },
            AppError::CompletedOutputUnavailable => Self {
                code: "invalid_path".into(),
                message: "Item gone".into(),
                retryable: false,
                field: None,
            },
            AppError::DesktopActionFailed => Self {
                code: "internal".into(),
                message: "Action failed".into(),
                retryable: true,
                field: None,
            },
            AppError::ClipboardBusy => Self {
                code: "clipboard_busy".into(),
                message: "Clipboard busy".into(),
                retryable: true,
                field: None,
            },
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn errors_are_stable_and_safe() {
        let dto: AppErrorDto = AppError::DataDirectoryUnavailable.into();
        assert_eq!(dto.code, "internal");
        assert!(!dto.retryable);
    }
    #[test]
    fn validation_preserves_only_the_stable_contract() {
        let dto: AppErrorDto = AppError::Validation {
            code: "invalid_path",
            message: "File or folder not found",
            field: Some("paths"),
        }
        .into();
        assert_eq!(dto.code, "invalid_path");
        assert_eq!(dto.field.as_deref(), Some("paths"));
    }
}
