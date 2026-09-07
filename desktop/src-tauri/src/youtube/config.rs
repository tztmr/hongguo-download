use super::models::CredentialSummary;
use crate::{platform_fs::replace_file, AppError};
use serde::Deserialize;
use serde_json::Value;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::{
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
};
use url::Url;

#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct OAuthClientConfig {
    pub client_id: String,
    pub client_secret: Option<SecretString>,
    pub auth_uri: String,
    pub token_uri: String,
    pub redirect_uris: Vec<String>,
}

impl fmt::Debug for OAuthClientConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthClientConfig")
            .field("client_id_suffix", &self.client_id_suffix())
            .field("client_secret", &self.client_secret)
            .field("auth_host", &safe_url_label(&self.auth_uri))
            .field("token_host", &safe_url_label(&self.token_uri))
            .field("redirect_uri_count", &self.redirect_uris.len())
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct ImportedCredential {
    pub path: PathBuf,
    pub summary: CredentialSummary,
}

impl OAuthClientConfig {
    pub fn load(path: &Path) -> Result<Self, AppError> {
        let bytes = fs::read(path)
            .map_err(|_| AppError::new("OAUTH_CONFIG_READ_FAILED", "无法读取 OAuth 凭证文件"))?;
        let json = std::str::from_utf8(&bytes)
            .map_err(|_| AppError::new("OAUTH_CONFIG_INVALID", "OAuth 凭证 JSON 编码无效"))?;
        Self::from_json(json)
    }

    pub fn from_json(json: &str) -> Result<Self, AppError> {
        let document: Value = serde_json::from_str(json)
            .map_err(|_| AppError::new("OAUTH_CONFIG_INVALID", "OAuth 凭证 JSON 格式无效"))?;
        let object = document
            .as_object()
            .ok_or_else(|| AppError::new("OAUTH_CONFIG_INVALID", "OAuth 凭证格式无效"))?;
        let installed = object
            .get("installed")
            .ok_or_else(desktop_credentials_required)?;
        if object.len() != 1 || !installed.is_object() {
            return Err(desktop_credentials_required());
        }
        let credentials: InstalledCredentials = serde_json::from_value(installed.clone())
            .map_err(|_| AppError::new("OAUTH_CONFIG_INVALID", "OAuth 凭证字段格式无效"))?;
        let client_id = required_non_empty(credentials.client_id)?;
        let auth_uri = required_non_empty(credentials.auth_uri)?;
        if !is_official_https_url(&auth_uri, "accounts.google.com") {
            return Err(AppError::new(
                "OAUTH_AUTH_URI_INVALID",
                "OAuth 授权地址必须使用 Google 官方地址",
            ));
        }
        let token_uri = required_non_empty(credentials.token_uri)?;
        if !is_official_https_url(&token_uri, "oauth2.googleapis.com") {
            return Err(AppError::new(
                "OAUTH_TOKEN_URI_INVALID",
                "OAuth 令牌地址必须使用 Google 官方地址",
            ));
        }
        let redirect_uris = credentials.redirect_uris.unwrap_or_default();
        if !redirect_uris.iter().any(|uri| is_loopback_redirect(uri)) {
            return Err(AppError::new(
                "OAUTH_REDIRECT_URI_INVALID",
                "OAuth 凭证必须包含本机回调地址",
            ));
        }
        Ok(Self {
            client_id,
            client_secret: credentials
                .client_secret
                .filter(|value| !value.trim().is_empty())
                .map(SecretString::new),
            auth_uri,
            token_uri,
            redirect_uris,
        })
    }

    pub fn client_id_suffix(&self) -> String {
        let chars = self.client_id.chars().collect::<Vec<_>>();
        if chars.len() <= 6 {
            "…".into()
        } else {
            format!("…{}", chars[chars.len() - 6..].iter().collect::<String>())
        }
    }

    pub fn summary(&self) -> CredentialSummary {
        CredentialSummary {
            configured: true,
            client_id_suffix: self.client_id_suffix(),
        }
    }
}

pub fn import_private(
    source: &Path,
    app_config_dir: &Path,
) -> Result<ImportedCredential, AppError> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|_| AppError::new("OAUTH_CONFIG_READ_FAILED", "无法读取 OAuth 凭证文件"))?;
    if unsafe_credential_metadata(&metadata) || !metadata.is_file() {
        return Err(AppError::new(
            "OAUTH_CONFIG_INVALID",
            "OAuth 凭证源文件不安全",
        ));
    }
    let bytes = fs::read(source)
        .map_err(|_| AppError::new("OAUTH_CONFIG_READ_FAILED", "无法读取 OAuth 凭证文件"))?;
    if bytes.len() > 64 * 1024 {
        return Err(AppError::new("OAUTH_CONFIG_INVALID", "OAuth 凭证文件过大"));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| AppError::new("OAUTH_CONFIG_INVALID", "OAuth 凭证 JSON 编码无效"))?;
    let config = OAuthClientConfig::from_json(text)?;
    let directory = app_config_dir.join("youtube");
    fs::create_dir_all(&directory).map_err(|error| {
        AppError::with_cause(
            "OAUTH_CONFIG_WRITE_FAILED",
            "无法创建 OAuth 私有目录",
            error.to_string(),
        )
    })?;
    protect_private_path(&directory, true)?;
    let target = directory.join("oauth-client.json");
    let temporary = directory.join("oauth-client.json.tmp");
    let mut file = private_write_options().open(&temporary).map_err(|error| {
        AppError::with_cause(
            "OAUTH_CONFIG_WRITE_FAILED",
            "无法保存 OAuth 凭证",
            error.to_string(),
        )
    })?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            AppError::with_cause(
                "OAUTH_CONFIG_WRITE_FAILED",
                "无法保存 OAuth 凭证",
                error.to_string(),
            )
        })?;
    drop(file);
    replace_file(&temporary, &target).map_err(|error| {
        AppError::with_cause(
            "OAUTH_CONFIG_WRITE_FAILED",
            "无法发布 OAuth 凭证",
            error.to_string(),
        )
    })?;
    protect_private_path(&target, false)?;
    Ok(ImportedCredential {
        path: target,
        summary: config.summary(),
    })
}

#[cfg(unix)]
fn unsafe_credential_metadata(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink() || metadata.nlink() != 1
}

#[cfg(windows)]
fn unsafe_credential_metadata(metadata: &fs::Metadata) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn private_write_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
}

#[cfg(unix)]
fn protect_private_path(path: &Path, directory: bool) -> Result<(), AppError> {
    let mode = if directory { 0o700 } else { 0o600 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|error| {
        AppError::with_cause(
            "OAUTH_CONFIG_WRITE_FAILED",
            "无法保护 OAuth 私有文件",
            error.to_string(),
        )
    })
}

#[cfg(windows)]
fn protect_private_path(_path: &Path, _directory: bool) -> Result<(), AppError> {
    // The application config directory inherits the current user's private ACL.
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstalledCredentials {
    client_id: Option<String>,
    client_secret: Option<String>,
    auth_uri: Option<String>,
    token_uri: Option<String>,
    redirect_uris: Option<Vec<String>>,
    #[serde(rename = "auth_provider_x509_cert_url")]
    _auth_provider_x509_cert_url: Option<String>,
    #[serde(rename = "project_id")]
    _project_id: Option<String>,
}

fn required_non_empty(value: Option<String>) -> Result<String, AppError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::new("OAUTH_CONFIG_INVALID", "OAuth 凭证缺少必需字段"))
}

fn is_official_https_url(value: &str, host: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some(host)
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

fn is_loopback_redirect(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1"))
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

fn safe_url_label(value: &str) -> String {
    Url::parse(value)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| format!("{}://{host}", url.scheme()))
        })
        .unwrap_or_else(|| "[INVALID]".into())
}

fn desktop_credentials_required() -> AppError {
    AppError::new(
        "OAUTH_DESKTOP_CREDENTIAL_REQUIRED",
        "请选择 Google Desktop OAuth 凭证",
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn valid_json(secret: &str) -> String {
        format!(
            r#"{{"installed":{{"client_id":"synthetic.apps.googleusercontent.com","client_secret":"{secret}","auth_uri":"https://accounts.google.com/o/oauth2/auth","token_uri":"https://oauth2.googleapis.com/token","redirect_uris":["http://127.0.0.1"]}}}}"#
        )
    }

    #[test]
    fn import_copies_only_valid_desktop_credentials_with_mode_0600() {
        let root =
            std::env::temp_dir().join(format!("hongguo-oauth-config-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.json");
        fs::write(&source, valid_json("synthetic-secret-value")).unwrap();
        let imported = import_private(&source, &root.join("private")).unwrap();
        assert_eq!(
            fs::metadata(&imported.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(imported.summary.client_id_suffix, "…nt.com");
        assert!(!format!("{:?}", imported.summary).contains("synthetic-secret-value"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_non_google_hosts_and_mixed_web_credentials() {
        let evil =
            valid_json("secret").replace("accounts.google.com/", "accounts.google.com.evil.test/");
        assert_eq!(
            OAuthClientConfig::from_json(&evil).unwrap_err().code,
            "OAUTH_AUTH_URI_INVALID"
        );
        assert_eq!(
            OAuthClientConfig::from_json(r#"{"web":{}}"#)
                .unwrap_err()
                .code,
            "OAUTH_DESKTOP_CREDENTIAL_REQUIRED"
        );
    }

    #[test]
    fn accepts_standard_google_desktop_metadata_fields() {
        let json = r#"{"installed":{"client_id":"synthetic.apps.googleusercontent.com","client_secret":"secret","auth_uri":"https://accounts.google.com/o/oauth2/auth","token_uri":"https://oauth2.googleapis.com/token","redirect_uris":["http://localhost"],"project_id":"synthetic-project","auth_provider_x509_cert_url":"https://www.googleapis.com/oauth2/v1/certs"}}"#;
        let config = OAuthClientConfig::from_json(json).unwrap();
        assert_eq!(config.client_id_suffix(), "…nt.com");
    }
}
