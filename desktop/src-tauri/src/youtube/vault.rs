use super::config::SecretString;
use crate::AppError;
use std::{collections::HashMap, sync::RwLock};

pub const KEYRING_SERVICE: &str = "com.edking.hongguo.desktop.youtube.refresh-token";

pub trait TokenVault: Send + Sync {
    fn save_refresh_token(&self, channel_id: &str, token: &str) -> Result<(), AppError>;
    fn load_refresh_token(&self, channel_id: &str) -> Result<SecretString, AppError>;
    fn delete_refresh_token(&self, channel_id: &str) -> Result<(), AppError>;
}

#[derive(Default)]
pub struct OsTokenVault;

impl TokenVault for OsTokenVault {
    fn save_refresh_token(&self, channel_id: &str, token: &str) -> Result<(), AppError> {
        keyring::Entry::new(KEYRING_SERVICE, channel_id)
            .map_err(map_keyring_error)?
            .set_password(token)
            .map_err(map_keyring_error)
    }

    fn load_refresh_token(&self, channel_id: &str) -> Result<SecretString, AppError> {
        keyring::Entry::new(KEYRING_SERVICE, channel_id)
            .map_err(map_keyring_error)?
            .get_password()
            .map(SecretString::new)
            .map_err(map_keyring_error)
    }

    fn delete_refresh_token(&self, channel_id: &str) -> Result<(), AppError> {
        keyring::Entry::new(KEYRING_SERVICE, channel_id)
            .map_err(map_keyring_error)?
            .delete_credential()
            .map_err(map_keyring_error)
    }
}

#[derive(Default)]
pub struct MemoryTokenVault {
    tokens: RwLock<HashMap<String, SecretString>>,
}

impl TokenVault for MemoryTokenVault {
    fn save_refresh_token(&self, channel_id: &str, token: &str) -> Result<(), AppError> {
        self.tokens
            .write()
            .map_err(|_| credential_vault_error())?
            .insert(channel_id.into(), SecretString::new(token));
        Ok(())
    }

    fn load_refresh_token(&self, channel_id: &str) -> Result<SecretString, AppError> {
        self.tokens
            .read()
            .map_err(|_| credential_vault_error())?
            .get(channel_id)
            .cloned()
            .ok_or_else(auth_required)
    }

    fn delete_refresh_token(&self, channel_id: &str) -> Result<(), AppError> {
        if self
            .tokens
            .write()
            .map_err(|_| credential_vault_error())?
            .remove(channel_id)
            .is_some()
        {
            Ok(())
        } else {
            Err(auth_required())
        }
    }
}

fn map_keyring_error(error: keyring::Error) -> AppError {
    if matches!(error, keyring::Error::NoEntry) {
        auth_required()
    } else {
        credential_vault_error()
    }
}

fn auth_required() -> AppError {
    AppError::new("AUTH_REQUIRED", "需要重新授权 YouTube 频道")
}

fn credential_vault_error() -> AppError {
    AppError::new("CREDENTIAL_VAULT_ERROR", "无法访问系统凭据保险库")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_vault_round_trip_delete_and_redacted_debug() {
        let vault = MemoryTokenVault::default();
        vault
            .save_refresh_token("UC_TEST", "synthetic-refresh-token")
            .unwrap();
        let token = vault.load_refresh_token("UC_TEST").unwrap();
        assert_eq!(token.expose_secret(), "synthetic-refresh-token");
        assert!(!format!("{token:?}").contains("synthetic-refresh-token"));
        vault.delete_refresh_token("UC_TEST").unwrap();
        assert_eq!(
            vault.load_refresh_token("UC_TEST").unwrap_err().code,
            "AUTH_REQUIRED"
        );
    }
}
