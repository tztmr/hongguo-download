use super::model::{fingerprint, text, KeyStatus};
use crate::AppError;
use serde::Deserialize;
use serde_json::Value;
const SERVICE: &str = "com.edking.hongguo.desktop.automation.ai";
#[derive(Default, Deserialize)]
pub struct Updates {
    pub text: Option<String>,
    pub image: Option<String>,
}
fn entry(config: &Value, kind: &str) -> Result<keyring::Entry, AppError> {
    let provider = text(
        config,
        if kind == "text" {
            "metadataSource"
        } else {
            "coverSource"
        },
    );
    let base = if provider == "jucodex" {
        text(config, "jucodexBaseUrl")
    } else {
        ""
    };
    keyring::Entry::new(
        SERVICE,
        &format!("{kind}-{}", fingerprint(&format!("{provider}|{base}"))),
    )
    .map_err(|_| error())
}
pub fn read(config: &Value, kind: &str) -> Result<Option<String>, AppError> {
    match entry(config, kind)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err(error()),
    }
}
pub fn update(config: &Value, updates: Updates) -> Result<KeyStatus, AppError> {
    for (kind, value) in [("text", updates.text), ("image", updates.image)] {
        if let Some(value) = value {
            let e = entry(config, kind)?;
            if value.trim().is_empty() {
                match e.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => {}
                    Err(_) => return Err(error()),
                }
            } else {
                e.set_password(value.trim()).map_err(|_| error())?;
            }
        }
    }
    Ok(status(config))
}
pub fn status(config: &Value) -> KeyStatus {
    KeyStatus {
        text: read(config, "text").ok().flatten().is_some(),
        image: read(config, "image").ok().flatten().is_some(),
    }
}
fn error() -> AppError {
    AppError::new(
        "AUTOMATION_KEY_VAULT_ERROR",
        "无法访问系统凭据库，未保存 AI Key；请检查系统权限后重试",
    )
}
