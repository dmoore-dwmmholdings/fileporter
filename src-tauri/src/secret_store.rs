//! Platform secret-store boundary.  Values are private key material and are
//! deliberately opaque to callers; neither implementations nor errors log it.

#[cfg(test)]
use std::{collections::HashMap, sync::Mutex};

use crate::error::AppError;

pub(crate) const SERVICE: &str = "io.fileporter.desktop";
pub(crate) const DEVICE_SCOPE: &str = "local-device-v1";

pub(crate) trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, AppError>;
    fn set(&self, name: &str, value: &[u8]) -> Result<(), AppError>;
}

/// `keyring` uses Windows Credential Manager and macOS Keychain for the
/// supported desktop targets.  The stable service/account pair deliberately
/// contains no device name, path, certificate, or key material.
pub(crate) struct PlatformSecretStore;

impl PlatformSecretStore {
    fn entry(name: &str) -> Result<keyring::Entry, AppError> {
        keyring::Entry::new(SERVICE, &format!("{DEVICE_SCOPE}.{name}"))
            .map_err(|_| AppError::IdentityStorageInvalid)
    }
}

impl SecretStore for PlatformSecretStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, AppError> {
        match Self::entry(name)?.get_password() {
            Ok(encoded) => hex::decode(encoded)
                .map(Some)
                .map_err(|_| AppError::IdentityStorageInvalid),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(AppError::IdentityStorageInvalid),
        }
    }

    fn set(&self, name: &str, value: &[u8]) -> Result<(), AppError> {
        Self::entry(name)?
            .set_password(&hex::encode(value))
            .map_err(|_| AppError::IdentityStorageInvalid)
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct InMemorySecretStore {
    values: Mutex<HashMap<String, Vec<u8>>>,
    fail_writes: Mutex<bool>,
}

#[cfg(test)]
impl InMemorySecretStore {
    pub(crate) fn fail_writes(&self) {
        *self
            .fail_writes
            .lock()
            .expect("secret store mutex poisoned") = true;
    }
}

#[cfg(test)]
impl SecretStore for InMemorySecretStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, AppError> {
        Ok(self
            .values
            .lock()
            .expect("secret store mutex poisoned")
            .get(name)
            .cloned())
    }

    fn set(&self, name: &str, value: &[u8]) -> Result<(), AppError> {
        if *self
            .fail_writes
            .lock()
            .expect("secret store mutex poisoned")
        {
            return Err(AppError::IdentityStorageInvalid);
        }
        self.values
            .lock()
            .expect("secret store mutex poisoned")
            .insert(name.into(), value.to_vec());
        Ok(())
    }
}
