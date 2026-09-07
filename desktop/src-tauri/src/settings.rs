use crate::platform_fs::replace_file;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use url::Url;

pub const SETTINGS_VERSION: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefinitionPreference {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "720p")]
    P720,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DemucsModel {
    #[serde(rename = "htdemucs")]
    HtDemucs,
    #[serde(rename = "htdemucs_ft")]
    HtDemucsFt,
}

impl DemucsModel {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "htdemucs" => Ok(Self::HtDemucs),
            "htdemucs_ft" => Ok(Self::HtDemucsFt),
            _ => Err(format!("不支持的 Demucs 模型: {value}")),
        }
    }

    #[cfg(test)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HtDemucs => "htdemucs",
            Self::HtDemucsFt => "htdemucs_ft",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WhisperModel {
    #[serde(rename = "small")]
    Small,
    #[serde(rename = "medium")]
    Medium,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AIDevicePreference {
    Auto,
    Cpu,
    Cuda,
}

impl AIDevicePreference {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "cpu" => Ok(Self::Cpu),
            "cuda" => Ok(Self::Cuda),
            _ => Err(format!("不支持的 AI 计算设备: {value}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cpu => "cpu",
            Self::Cuda => "cuda",
        }
    }
}

impl WhisperModel {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "small" => Ok(Self::Small),
            "medium" => Ok(Self::Medium),
            _ => Err(format!("不支持的 Whisper 模型: {value}")),
        }
    }

    #[cfg(test)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
        }
    }
}
impl DefinitionPreference {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "1080p" => Ok(Self::P1080),
            "720p" => Ok(Self::P720),
            _ => Err(format!("不支持的分辨率: {value}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::P1080 => "1080p",
            Self::P720 => "720p",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub version: u8,
    pub save_dir: PathBuf,
    pub definition: DefinitionPreference,
    pub notify_download_complete: bool,
    pub notify_new_releases: bool,
    #[serde(default = "default_true")]
    pub notify_media_complete: bool,
    #[serde(default = "default_true")]
    pub notify_youtube_result: bool,
    #[serde(default = "default_demucs_model")]
    pub demucs_model: DemucsModel,
    #[serde(default = "default_whisper_model")]
    pub whisper_model: WhisperModel,
    #[serde(default = "default_ai_device")]
    pub ai_device: AIDevicePreference,
    /// Optional proxy for upstream API and component downloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_proxy: Option<String>,
    /// Optional HTTPS mirror containing the component archives by filename.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_mirror: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

impl AppSettings {
    pub fn default_for(save_dir: PathBuf) -> Self {
        Self {
            version: SETTINGS_VERSION,
            save_dir,
            definition: DefinitionPreference::Auto,
            notify_download_complete: true,
            notify_new_releases: true,
            notify_media_complete: true,
            notify_youtube_result: true,
            demucs_model: DemucsModel::HtDemucs,
            whisper_model: WhisperModel::Small,
            ai_device: AIDevicePreference::Auto,
            download_proxy: None,
            download_mirror: None,
            warning: None,
        }
    }

    pub fn apply(&mut self, patch: UpdateSettings) -> Result<(), String> {
        let mut next = self.clone();
        if let Some(save_dir) = patch.save_dir {
            if save_dir.trim().is_empty() {
                return Err("下载目录不能为空".into());
            }
            next.save_dir = PathBuf::from(save_dir);
        }
        if let Some(definition) = patch.definition {
            next.definition = DefinitionPreference::parse(&definition)?;
        }
        if let Some(value) = patch.notify_download_complete {
            next.notify_download_complete = value;
        }
        if let Some(value) = patch.notify_new_releases {
            next.notify_new_releases = value;
        }
        if let Some(value) = patch.notify_media_complete {
            next.notify_media_complete = value;
        }
        if let Some(value) = patch.notify_youtube_result {
            next.notify_youtube_result = value;
        }
        if let Some(value) = patch.demucs_model {
            next.demucs_model = DemucsModel::parse(&value)?;
        }
        if let Some(value) = patch.whisper_model {
            next.whisper_model = WhisperModel::parse(&value)?;
        }
        if let Some(value) = patch.ai_device {
            next.ai_device = AIDevicePreference::parse(&value)?;
        }
        if let Some(value) = patch.download_proxy {
            next.download_proxy = normalize_proxy(&value)?;
        }
        if let Some(value) = patch.download_mirror {
            next.download_mirror = normalize_mirror(&value)?;
        }
        next.version = SETTINGS_VERSION;
        next.warning = None;
        *self = next;
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettings {
    pub save_dir: Option<String>,
    pub definition: Option<String>,
    pub notify_download_complete: Option<bool>,
    pub notify_new_releases: Option<bool>,
    pub notify_media_complete: Option<bool>,
    pub notify_youtube_result: Option<bool>,
    pub demucs_model: Option<String>,
    pub whisper_model: Option<String>,
    pub ai_device: Option<String>,
    /// Empty string clears the proxy. Supported schemes: http, https, socks5, socks5h.
    pub download_proxy: Option<String>,
    /// Empty string clears the component mirror. Must be an HTTPS directory URL.
    pub download_mirror: Option<String>,
}

fn normalize_proxy(value: &str) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let parsed = Url::parse(value).map_err(|_| {
        "代理地址无效，请填写 http://、https://、socks5:// 或 socks5h:// 地址".to_string()
    })?;
    if !matches!(parsed.scheme(), "http" | "https" | "socks5" | "socks5h")
        || parsed.host_str().is_none()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("代理地址无效，请填写 http://、https://、socks5:// 或 socks5h:// 地址".into());
    }
    Ok(Some(parsed.to_string()))
}

fn normalize_mirror(value: &str) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let parsed = Url::parse(value)
        .map_err(|_| "国内镜像地址无效，请填写公开的 HTTPS 目录地址".to_string())?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("国内镜像地址无效，请填写公开的 HTTPS 目录地址".into());
    }
    Ok(Some(parsed.to_string()))
}

pub fn default_demucs_model() -> DemucsModel {
    DemucsModel::HtDemucs
}
fn default_whisper_model() -> WhisperModel {
    WhisperModel::Small
}
fn default_ai_device() -> AIDevicePreference {
    AIDevicePreference::Auto
}
fn default_true() -> bool {
    true
}

fn migrate_settings(mut settings: AppSettings, default_save_dir: PathBuf) -> AppSettings {
    let mut invalid_network = false;
    if settings.save_dir.as_os_str().is_empty() {
        settings.save_dir = default_save_dir;
    }
    if let Some(value) = settings.download_proxy.clone() {
        match normalize_proxy(&value) {
            Ok(normalized) => settings.download_proxy = normalized,
            Err(_) => {
                settings.download_proxy = None;
                invalid_network = true;
            }
        }
    }
    if let Some(value) = settings.download_mirror.clone() {
        match normalize_mirror(&value) {
            Ok(normalized) => settings.download_mirror = normalized,
            Err(_) => {
                settings.download_mirror = None;
                invalid_network = true;
            }
        }
    }
    settings.version = SETTINGS_VERSION;
    settings.warning = invalid_network.then(|| "下载网络设置无效，已恢复直连".into());
    settings
}

pub fn load_settings(path: &Path, default_save_dir: PathBuf) -> AppSettings {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return AppSettings::default_for(default_save_dir)
        }
        Err(error) => {
            let mut fallback = AppSettings::default_for(default_save_dir);
            fallback.warning = Some(format!("读取设置文件失败: {error}"));
            return fallback;
        }
    };
    let mut value = match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(value) => value,
        Err(error) => {
            let mut fallback = AppSettings::default_for(default_save_dir);
            fallback.warning = Some(format!("设置文件损坏，已使用默认值: {error}"));
            return fallback;
        }
    };
    let mut invalid_ai_model = false;
    let mut invalid_ai_device = false;
    if let Some(object) = value.as_object_mut() {
        let invalid_demucs = object.get("demucsModel").is_some_and(|stored| {
            stored
                .as_str()
                .is_none_or(|value| !matches!(value, "htdemucs" | "htdemucs_ft"))
        });
        if invalid_demucs {
            object.insert(
                "demucsModel".into(),
                serde_json::Value::String("htdemucs".into()),
            );
            invalid_ai_model = true;
        }
        let invalid_whisper = object.get("whisperModel").is_some_and(|stored| {
            stored
                .as_str()
                .is_none_or(|value| !matches!(value, "small" | "medium"))
        });
        if invalid_whisper {
            object.insert(
                "whisperModel".into(),
                serde_json::Value::String("small".into()),
            );
            invalid_ai_model = true;
        }
        let invalid_device = object.get("aiDevice").is_some_and(|stored| {
            stored
                .as_str()
                .is_none_or(|value| !matches!(value, "auto" | "cpu" | "cuda"))
        });
        if invalid_device {
            object.insert("aiDevice".into(), serde_json::Value::String("auto".into()));
            invalid_ai_device = true;
        }
    }
    match serde_json::from_value::<AppSettings>(value) {
        Ok(settings) if matches!(settings.version, 1 | 2 | 3 | SETTINGS_VERSION) => {
            let mut settings = migrate_settings(settings, default_save_dir);
            if invalid_ai_model {
                settings.warning = Some(match settings.warning.take() {
                    Some(existing) => format!("{existing}；AI 模型设置无效，已恢复默认模型"),
                    None => "AI 模型设置无效，已恢复默认模型".into(),
                });
            }
            if invalid_ai_device {
                settings.warning = Some(match settings.warning.take() {
                    Some(existing) => format!("{existing}；AI 计算设备设置无效，已恢复自动选择"),
                    None => "AI 计算设备设置无效，已恢复自动选择".into(),
                });
            }
            settings
        }
        Ok(_) => {
            let mut fallback = AppSettings::default_for(default_save_dir);
            fallback.warning = Some("设置文件版本不兼容，已使用默认值".into());
            fallback
        }
        Err(error) => {
            let mut fallback = AppSettings::default_for(default_save_dir);
            fallback.warning = Some(format!("设置文件损坏，已使用默认值: {error}"));
            fallback
        }
    }
}

pub fn save_settings(path: &Path, settings: &AppSettings) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "设置文件路径无效".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("创建设置目录失败: {error}"))?;
    let temp_path = path.with_extension("json.tmp");
    let bytes =
        serde_json::to_vec_pretty(settings).map_err(|error| format!("序列化设置失败: {error}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp_path)
        .map_err(|error| format!("写入设置失败: {error}"))?;
    file.write_all(&bytes)
        .map_err(|error| format!("写入设置失败: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("同步设置失败: {error}"))?;
    drop(file);
    replace_file(&temp_path, path).map_err(|error| format!("保存设置失败: {error}"))?;
    if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_dir(name: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("hongguo-settings-{name}-{nonce}"))
    }

    #[test]
    fn v1_settings_migrate_to_default_ai_models() {
        // Production mutation caught: dropping saveDir/definition while adding AI model fields.
        let root = test_dir("v1-migrate");
        let path = root.join("settings.json");
        fs::create_dir_all(&root).expect("fixture");
        fs::write(
            &path,
            br#"{"version":1,"saveDir":"/tmp/keep","definition":"720p","notifyDownloadComplete":false,"notifyNewReleases":false}"#,
        )
        .expect("write v1");
        let restored = load_settings(&path, std::path::PathBuf::from("/tmp/default"));
        assert_eq!(restored.version, 4);
        assert_eq!(restored.save_dir, std::path::PathBuf::from("/tmp/keep"));
        assert_eq!(restored.definition, DefinitionPreference::P720);
        assert!(!restored.notify_download_complete);
        assert!(!restored.notify_new_releases);
        assert!(restored.notify_media_complete);
        assert!(restored.notify_youtube_result);
        assert_eq!(restored.demucs_model, DemucsModel::HtDemucs);
        assert_eq!(restored.whisper_model, WhisperModel::Small);
        assert_eq!(restored.demucs_model.as_str(), "htdemucs");
        assert_eq!(restored.whisper_model.as_str(), "small");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn v2_settings_migrate_to_enabled_media_and_youtube_notifications() {
        let root = test_dir("v2-notification-migrate");
        let path = root.join("settings.json");
        fs::create_dir_all(&root).expect("fixture");
        fs::write(
            &path,
            br#"{"version":2,"saveDir":"/tmp/keep","definition":"1080p","notifyDownloadComplete":false,"notifyNewReleases":true,"demucsModel":"htdemucs_ft","whisperModel":"medium"}"#,
        )
        .expect("write v2");
        let restored = load_settings(&path, std::path::PathBuf::from("/tmp/default"));
        assert_eq!(restored.version, 4);
        assert!(restored.notify_media_complete);
        assert!(restored.notify_youtube_result);
        assert!(!restored.notify_download_complete);
        assert_eq!(restored.demucs_model, DemucsModel::HtDemucsFt);
        assert_eq!(restored.whisper_model, WhisperModel::Medium);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_settings_use_defaults() {
        let root = test_dir("missing");
        let path = root.join("settings.json");
        let settings = load_settings(&path, std::path::PathBuf::from("/tmp/default"));

        assert_eq!(settings.definition, DefinitionPreference::Auto);
        assert_eq!(settings.save_dir, std::path::PathBuf::from("/tmp/default"));
        assert!(settings.notify_download_complete);
        assert!(settings.notify_new_releases);
        assert_eq!(settings.ai_device.as_str(), "auto");
        assert!(settings.warning.is_none());
    }

    #[test]
    fn ai_device_patch_accepts_supported_values_and_rejects_unknown_values() {
        let mut settings = AppSettings::default_for(std::path::PathBuf::from("/tmp/default"));

        settings
            .apply(UpdateSettings {
                ai_device: Some("cuda".into()),
                ..Default::default()
            })
            .expect("CUDA preference");
        assert_eq!(settings.ai_device.as_str(), "cuda");

        let result = settings.apply(UpdateSettings {
            ai_device: Some("directml".into()),
            ..Default::default()
        });
        assert!(result.is_err());
        assert_eq!(settings.ai_device.as_str(), "cuda");
    }

    #[test]
    fn invalid_definition_is_rejected_without_mutating_settings() {
        let mut settings = AppSettings::default_for(std::path::PathBuf::from("/tmp/default"));

        let result = settings.apply(UpdateSettings {
            definition: Some("4k".into()),
            ..Default::default()
        });

        assert!(result.is_err());
        assert_eq!(settings.definition, DefinitionPreference::Auto);
    }

    #[test]
    fn invalid_stored_ai_models_preserve_unrelated_settings_and_warn() {
        // Production mutation caught: strict enum deserialization falling back to all defaults.
        let root = test_dir("invalid-ai-models");
        let path = root.join("settings.json");
        fs::create_dir_all(&root).expect("fixture");
        fs::write(
            &path,
            br#"{"version":2,"saveDir":"/tmp/keep","definition":"720p","notifyDownloadComplete":false,"notifyNewReleases":false,"demucsModel":"unknown-demucs","whisperModel":"unknown-whisper"}"#,
        )
        .expect("write settings");

        let restored = load_settings(&path, std::path::PathBuf::from("/tmp/default"));

        assert_eq!(restored.save_dir, std::path::PathBuf::from("/tmp/keep"));
        assert_eq!(restored.definition, DefinitionPreference::P720);
        assert!(!restored.notify_download_complete);
        assert!(!restored.notify_new_releases);
        assert_eq!(restored.demucs_model, DemucsModel::HtDemucs);
        assert_eq!(restored.whisper_model, WhisperModel::Small);
        assert!(restored
            .warning
            .as_deref()
            .unwrap_or("")
            .contains("AI 模型"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_ai_model_patch_is_rejected_without_mutating_settings() {
        // Production mutation caught: silently coercing unsupported model names to defaults.
        let mut settings = AppSettings::default_for(std::path::PathBuf::from("/tmp/default"));
        settings.definition = DefinitionPreference::P720;

        let result = settings.apply(UpdateSettings {
            demucs_model: Some("not-a-model".into()),
            ..Default::default()
        });

        assert!(result.is_err());
        assert_eq!(settings.definition, DefinitionPreference::P720);
        assert_eq!(settings.demucs_model, DemucsModel::HtDemucs);
    }

    #[test]
    fn network_settings_accept_supported_proxy_and_https_mirror_and_can_clear_them() {
        let mut settings = AppSettings::default_for(std::path::PathBuf::from("/tmp/default"));
        settings
            .apply(UpdateSettings {
                download_proxy: Some("socks5://127.0.0.1:7890".into()),
                download_mirror: Some("https://mirror.example/ai".into()),
                ..Default::default()
            })
            .expect("valid network update");
        assert_eq!(
            settings.download_proxy.as_deref(),
            Some("socks5://127.0.0.1:7890")
        );
        assert_eq!(
            settings.download_mirror.as_deref(),
            Some("https://mirror.example/ai")
        );
        settings
            .apply(UpdateSettings {
                download_proxy: Some(String::new()),
                download_mirror: Some(String::new()),
                ..Default::default()
            })
            .expect("clearing network update");
        assert!(settings.download_proxy.is_none());
        assert!(settings.download_mirror.is_none());
    }

    #[test]
    fn network_settings_reject_unsafe_proxy_and_mirror() {
        let mut settings = AppSettings::default_for(std::path::PathBuf::from("/tmp/default"));
        assert!(settings
            .apply(UpdateSettings {
                download_proxy: Some("file:///tmp/proxy".into()),
                ..Default::default()
            })
            .is_err());
        assert!(settings
            .apply(UpdateSettings {
                download_mirror: Some("http://mirror.example/ai".into()),
                ..Default::default()
            })
            .is_err());
    }

    #[test]
    fn invalid_stored_network_settings_fall_back_to_direct_without_blocking_startup() {
        let root = test_dir("invalid-network");
        let path = root.join("settings.json");
        fs::create_dir_all(&root).expect("fixture");
        fs::write(
            &path,
            br#"{"version":3,"saveDir":"/tmp/keep","definition":"auto","notifyDownloadComplete":true,"notifyNewReleases":true,"downloadProxy":"file:///tmp/proxy","downloadMirror":"http://mirror.example/ai"}"#,
        )
        .expect("write settings");
        let restored = load_settings(&path, std::path::PathBuf::from("/tmp/default"));
        assert!(restored.download_proxy.is_none());
        assert!(restored.download_mirror.is_none());
        assert!(restored
            .warning
            .as_deref()
            .unwrap_or("")
            .contains("下载网络"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn settings_round_trip_and_corrupt_file_falls_back_without_deleting_source() {
        let root = test_dir("roundtrip");
        let path = root.join("settings.json");
        let mut settings = AppSettings::default_for(root.join("downloads"));
        settings
            .apply(UpdateSettings {
                definition: Some("720p".into()),
                notify_new_releases: Some(false),
                ..Default::default()
            })
            .expect("valid update");
        save_settings(&path, &settings).expect("save settings");

        let restored = load_settings(&path, root.join("fallback"));
        assert_eq!(restored.definition, DefinitionPreference::P720);
        assert!(!restored.notify_new_releases);

        fs::write(&path, b"not-json").expect("write corrupt file");
        let fallback = load_settings(&path, root.join("fallback"));
        assert_eq!(fallback.definition, DefinitionPreference::Auto);
        assert!(fallback
            .warning
            .as_deref()
            .unwrap_or("")
            .contains("设置文件"));
        assert_eq!(
            fs::read(&path).expect("corrupt source remains"),
            b"not-json"
        );
        let _ = fs::remove_dir_all(root);
    }
}
