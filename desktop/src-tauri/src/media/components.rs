use super::tools::background_command;
use crate::app_error::AppError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Read, Write},
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::Url;

pub const AI_COMPONENTS_MANIFEST_VERSION: u32 = 1;
const COMPONENT_DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const COMPONENT_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
#[cfg(target_os = "macos")]
pub const AI_COMPONENT_PLATFORM: &str = "aarch64-apple-darwin";
#[cfg(windows)]
pub const AI_COMPONENT_PLATFORM: &str = "x86_64-pc-windows-msvc";
const INSTALLED_STORE: &str = "installed.json";
const DOWNLOADS_DIR: &str = ".downloads";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentReleasePart {
    pub url: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentRelease {
    pub id: String,
    pub version: String,
    pub platform: String,
    pub url: String,
    pub sha256: String,
    pub download_bytes: u64,
    pub installed_bytes: u64,
    pub entrypoint: String,
    #[serde(default)]
    pub parts: Vec<ComponentReleasePart>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentManifest {
    pub version: u32,
    pub platform: String,
    pub components: Vec<ComponentRelease>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ComponentStatus {
    pub id: String,
    pub version: String,
    pub installed: bool,
    pub installed_version: Option<String>,
    pub download_bytes: u64,
    pub installed_bytes: u64,
    pub in_use: bool,
    pub installed_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ComponentProgress {
    pub id: String,
    pub stage: String,
    pub percent: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledComponent {
    pub root: PathBuf,
    pub entrypoint: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct InstalledRecord {
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct InstalledStore {
    pub version: u32,
    pub items: Vec<InstalledRecord>,
}

pub struct ComponentManager {
    root: PathBuf,
    manifest: ComponentManifest,
    installing: AtomicBool,
    in_use: Mutex<HashMap<String, usize>>,
    min_free_bytes: Mutex<Option<u64>>,
    download_proxy: Mutex<Option<Url>>,
    download_mirror: Mutex<Option<Url>>,
}

impl ComponentManager {
    pub fn load(root: impl AsRef<Path>, manifest: ComponentManifest) -> Result<Self, AppError> {
        validate_manifest(&manifest)?;
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join(DOWNLOADS_DIR)).map_err(|error| {
            AppError::with_cause("AI_COMPONENT_IO", "无法创建组件目录", error.to_string())
        })?;
        Ok(Self {
            root,
            manifest,
            installing: AtomicBool::new(false),
            in_use: Mutex::new(HashMap::new()),
            min_free_bytes: Mutex::new(None),
            download_proxy: Mutex::new(None),
            download_mirror: Mutex::new(None),
        })
    }

    pub fn from_manifest_json(root: impl AsRef<Path>, json: &str) -> Result<Self, AppError> {
        let manifest = parse_manifest(json)?;
        Self::load(root, manifest)
    }

    pub fn status(&self) -> Vec<ComponentStatus> {
        let installed = load_installed(&self.root).unwrap_or_default();
        let in_use = self
            .in_use
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        self.manifest
            .components
            .iter()
            .map(|release| {
                let current = installed
                    .items
                    .iter()
                    .find(|item| item.id == release.id)
                    .map(|item| item.version.clone());
                let installed_now = current.as_deref() == Some(release.version.as_str());
                ComponentStatus {
                    id: release.id.clone(),
                    version: release.version.clone(),
                    installed: installed_now,
                    installed_version: current,
                    download_bytes: release.download_bytes,
                    installed_bytes: release.installed_bytes,
                    in_use: in_use.contains_key(&release.id),
                    installed_path: if installed_now {
                        Some(
                            self.root
                                .join(&release.id)
                                .join(&release.version)
                                .to_string_lossy()
                                .into_owned(),
                        )
                    } else {
                        None
                    },
                }
            })
            .collect()
    }

    pub fn mark_in_use(&self, id: &str, used: bool) {
        if let Ok(mut guard) = self.in_use.lock() {
            if used {
                *guard.entry(id.to_string()).or_default() += 1;
            } else if let Some(count) = guard.get_mut(id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    guard.remove(id);
                }
            }
        }
    }

    #[cfg(test)]
    pub fn set_min_free_bytes(&self, bytes: Option<u64>) {
        if let Ok(mut guard) = self.min_free_bytes.lock() {
            *guard = bytes;
        }
    }

    #[cfg(test)]
    pub fn force_installing(&self) {
        self.installing.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub fn poison_in_use(&self) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.in_use.lock().expect("in-use lock should be available");
            panic!("poison in_use lock");
        }));
    }

    pub fn installed_version(&self, id: &str) -> Option<String> {
        load_installed(&self.root)
            .ok()
            .and_then(|store| store.items.into_iter().find(|item| item.id == id))
            .map(|item| item.version)
    }

    /// Applies the user-selected network route to future component downloads.
    /// The API sidecar receives the same proxy at startup for video/API traffic.
    pub fn configure_network(
        &self,
        proxy: Option<&str>,
        mirror: Option<&str>,
    ) -> Result<(), AppError> {
        let proxy = proxy
            .map(Url::parse)
            .transpose()
            .map_err(|_| AppError::new("AI_COMPONENT_NETWORK_INVALID", "代理地址无效"))?;
        if let Some(url) = &proxy {
            if !is_allowed_proxy_url(url) {
                return Err(AppError::new(
                    "AI_COMPONENT_NETWORK_INVALID",
                    "代理地址无效，请填写 http://、https://、socks5:// 或 socks5h:// 地址",
                ));
            }
        }
        let mirror = mirror
            .map(Url::parse)
            .transpose()
            .map_err(|_| AppError::new("AI_COMPONENT_NETWORK_INVALID", "国内镜像地址无效"))?;
        if let Some(url) = &mirror {
            if !is_allowed_mirror_url(url) {
                return Err(AppError::new(
                    "AI_COMPONENT_NETWORK_INVALID",
                    "国内镜像地址无效，请填写公开的 HTTPS 目录地址",
                ));
            }
        }
        *self
            .download_proxy
            .lock()
            .map_err(|_| AppError::new("AI_COMPONENT_NETWORK_INVALID", "无法更新下载网络设置"))? =
            proxy;
        *self
            .download_mirror
            .lock()
            .map_err(|_| AppError::new("AI_COMPONENT_NETWORK_INVALID", "无法更新下载网络设置"))? =
            mirror;
        Ok(())
    }

    pub fn resolve_installed(&self, id: &str) -> Result<InstalledComponent, AppError> {
        let release = self
            .manifest
            .components
            .iter()
            .find(|release| release.id == id)
            .ok_or_else(|| AppError::new("AI_COMPONENT_UNKNOWN", "未知的媒体组件"))?;
        if self.installed_version(id).as_deref() != Some(release.version.as_str()) {
            return Err(AppError::new(
                "AI_COMPONENT_NOT_INSTALLED",
                "AI 运行环境或所选模型尚未安装",
            ));
        }
        let root = self.root.join(id).join(&release.version);
        let canonical_root = fs::canonicalize(&root).map_err(|error| {
            AppError::with_cause("AI_COMPONENT_INVALID", "AI 组件不可用", error.to_string())
        })?;
        let entrypoint = root.join(&release.entrypoint);
        verify_entrypoint(&canonical_root, &entrypoint)?;
        Ok(InstalledComponent {
            root: canonical_root,
            entrypoint: fs::canonicalize(entrypoint).map_err(|error| {
                AppError::with_cause("AI_COMPONENT_INVALID", "AI 组件不可用", error.to_string())
            })?,
        })
    }

    pub fn remove(&self, id: &str) -> Result<(), AppError> {
        match self.in_use.lock() {
            Ok(guard) if guard.contains_key(id) => {
                return Err(AppError::new(
                    "AI_COMPONENT_IN_USE",
                    "运行中的媒体任务正在使用该组件",
                ));
            }
            Ok(_) => {}
            Err(_) => {
                return Err(AppError::new(
                    "AI_COMPONENT_IN_USE",
                    "运行中的媒体任务正在使用该组件",
                ));
            }
        }
        let mut store = load_installed(&self.root).unwrap_or_default();
        store.items.retain(|item| item.id != id);
        persist_installed(&self.root, &store)?;
        let target = self.root.join(id);
        if target.exists() {
            fs::remove_dir_all(&target).map_err(|error| {
                AppError::with_cause("AI_COMPONENT_IO", "无法删除组件", error.to_string())
            })?;
        }
        Ok(())
    }

    pub fn install(
        &self,
        id: &str,
        mut progress: impl FnMut(ComponentProgress),
    ) -> Result<ComponentStatus, AppError> {
        self.install_locked(id, &mut progress)
    }

    fn install_locked(
        &self,
        id: &str,
        progress: &mut dyn FnMut(ComponentProgress),
    ) -> Result<ComponentStatus, AppError> {
        if self.installing.swap(true, Ordering::SeqCst) {
            return Err(AppError::new(
                "AI_COMPONENT_INSTALL_RUNNING",
                "已有组件正在安装",
            ));
        }
        struct InstallGuard<'a> {
            flag: &'a AtomicBool,
        }
        impl Drop for InstallGuard<'_> {
            fn drop(&mut self) {
                self.flag.store(false, Ordering::SeqCst);
            }
        }
        let _guard = InstallGuard {
            flag: &self.installing,
        };
        let outcome = self.install_command_path(id, progress);
        if let Err(error) = &outcome {
            emit(progress, id, "failed", 100.0);
            let _ = error;
        }
        outcome
    }

    fn install_command_path(
        &self,
        id: &str,
        progress: &mut dyn FnMut(ComponentProgress),
    ) -> Result<ComponentStatus, AppError> {
        let release = self
            .manifest
            .components
            .iter()
            .find(|item| item.id == id)
            .cloned()
            .ok_or_else(|| AppError::new("AI_COMPONENT_UNKNOWN", "未知的媒体组件"))?;
        emit(progress, id, "checking", 5.0);
        let required = release.download_bytes.max(release.installed_bytes);
        let free = self.available_bytes()?;
        if free < required {
            return Err(AppError::new(
                "AI_COMPONENT_INSUFFICIENT_DISK",
                "磁盘空间不足，无法安装媒体组件",
            ));
        }
        emit(progress, id, "downloading", 20.0);
        let part_path = self.root.join(DOWNLOADS_DIR).join(format!("{id}.part"));
        let proxy = self
            .download_proxy
            .lock()
            .map_err(|_| AppError::new("AI_COMPONENT_DOWNLOAD_FAILED", "无法读取下载网络设置"))?
            .clone();
        let reuse_verified_part = fs::symlink_metadata(&part_path).is_ok_and(|metadata| {
            metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() == release.download_bytes
        }) && sha256_file(&part_path)
            .is_ok_and(|actual| actual == release.sha256);
        if !reuse_verified_part {
            let actual = if release.parts.is_empty() {
                stream_download(
                    &self.resolve_download_url(&release.url),
                    &part_path,
                    release.download_bytes,
                    id,
                    progress,
                    proxy.as_ref(),
                )?
            } else {
                let parts: Vec<(String, u64)> = release
                    .parts
                    .iter()
                    .map(|part| (self.resolve_download_url(&part.url), part.bytes))
                    .collect();
                stream_download_parts(
                    &parts,
                    &part_path,
                    release.download_bytes,
                    id,
                    progress,
                    proxy.as_ref(),
                )?
            };
            emit(progress, id, "verifying", 45.0);
            if actual != release.sha256 {
                let _ = fs::remove_file(&part_path);
                return Err(AppError::new(
                    "AI_COMPONENT_CHECKSUM_FAILED",
                    "组件校验失败，已保留当前版本",
                ));
            }
        } else {
            emit(progress, id, "verifying", 45.0);
        }
        self.finish_verified_archive(id, &release, &part_path, progress)
    }

    fn finish_verified_archive(
        &self,
        id: &str,
        release: &ComponentRelease,
        part_path: &Path,
        progress: &mut dyn FnMut(ComponentProgress),
    ) -> Result<ComponentStatus, AppError> {
        emit(progress, id, "extracting", 65.0);
        let staging = unique_dir(&self.root.join(DOWNLOADS_DIR), &format!("{id}-stage"));
        fs::create_dir_all(&staging).map_err(|error| {
            AppError::with_cause("AI_COMPONENT_IO", "无法创建组件解压目录", error.to_string())
        })?;
        struct StagingCleanup(PathBuf);
        impl Drop for StagingCleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _staging_cleanup = StagingCleanup(staging.clone());
        extract_archive(part_path, &staging)?;
        let entrypoint = staging.join(&release.entrypoint);
        verify_entrypoint(&staging, &entrypoint)?;
        emit(progress, id, "selfTesting", 85.0);
        run_self_test(&entrypoint)?;
        publish_atomically(&self.root, &release.id, &release.version, &staging)?;
        let _ = fs::remove_file(part_path);
        let mut store = load_installed(&self.root).unwrap_or_default();
        store.items.retain(|item| item.id != release.id);
        store.items.push(InstalledRecord {
            id: release.id.clone(),
            version: release.version.clone(),
        });
        persist_installed(&self.root, &store)?;
        emit(progress, id, "installed", 100.0);
        self.status()
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| AppError::new("AI_COMPONENT_IO", "组件安装记录缺失"))
    }

    pub fn install_bytes(
        &self,
        id: &str,
        bytes: &[u8],
        sha256: &str,
        mut progress: impl FnMut(ComponentProgress),
    ) -> Result<ComponentStatus, AppError> {
        if self.installing.swap(true, Ordering::SeqCst) {
            return Err(AppError::new(
                "AI_COMPONENT_INSTALL_RUNNING",
                "已有组件正在安装",
            ));
        }
        let result = self.install_bytes_inner(id, bytes, sha256, &mut progress);
        self.installing.store(false, Ordering::SeqCst);
        result
    }

    fn install_bytes_inner(
        &self,
        id: &str,
        bytes: &[u8],
        sha256: &str,
        progress: &mut dyn FnMut(ComponentProgress),
    ) -> Result<ComponentStatus, AppError> {
        let release = self
            .manifest
            .components
            .iter()
            .find(|item| item.id == id)
            .cloned()
            .ok_or_else(|| AppError::new("AI_COMPONENT_UNKNOWN", "未知的媒体组件"))?;
        emit(progress, id, "checking", 5.0);
        let required = release.download_bytes.max(release.installed_bytes);
        let free = self.available_bytes()?;
        if free < required {
            return Err(AppError::new(
                "AI_COMPONENT_INSUFFICIENT_DISK",
                "磁盘空间不足，无法安装媒体组件",
            ));
        }
        emit(progress, id, "downloading", 20.0);
        let part_path = self.root.join(DOWNLOADS_DIR).join(format!("{id}.part"));
        fs::write(&part_path, bytes).map_err(|error| {
            AppError::with_cause("AI_COMPONENT_IO", "无法写入组件下载文件", error.to_string())
        })?;
        emit(progress, id, "verifying", 45.0);
        let actual = sha256_file(&part_path)?;
        if actual != sha256 || actual != release.sha256 {
            let _ = fs::remove_file(&part_path);
            return Err(AppError::new(
                "AI_COMPONENT_CHECKSUM_FAILED",
                "组件校验失败，已保留当前版本",
            ));
        }
        self.finish_verified_archive(id, &release, &part_path, progress)
    }

    fn available_bytes(&self) -> Result<u64, AppError> {
        if let Ok(guard) = self.min_free_bytes.lock() {
            if let Some(forced) = *guard {
                return Ok(forced);
            }
        }
        fs2::available_space(&self.root).map_err(|error| {
            AppError::with_cause("AI_COMPONENT_IO", "无法读取磁盘空间", error.to_string())
        })
    }

    fn resolve_download_url(&self, original: &str) -> String {
        let mirror = self
            .download_mirror
            .lock()
            .ok()
            .and_then(|value| value.clone());
        let Some(mirror) = mirror else {
            return original.to_string();
        };
        let Ok(source) = Url::parse(original) else {
            return original.to_string();
        };
        let Some(filename) = source
            .path_segments()
            .and_then(|mut segments| segments.next_back())
        else {
            return original.to_string();
        };
        let mut path = mirror.path().to_string();
        if !path.ends_with('/') {
            path.push('/');
        }
        path.push_str(filename);
        let mut target = mirror;
        target.set_path(&path);
        target.set_query(None);
        target.set_fragment(None);
        target.to_string()
    }
}

pub fn parse_manifest(json: &str) -> Result<ComponentManifest, AppError> {
    let manifest: ComponentManifest = serde_json::from_str(json)
        .map_err(|_| AppError::new("AI_COMPONENT_MANIFEST_INVALID", "组件清单格式无效"))?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn validate_manifest(manifest: &ComponentManifest) -> Result<(), AppError> {
    if manifest.version != AI_COMPONENTS_MANIFEST_VERSION
        || manifest.platform != AI_COMPONENT_PLATFORM
    {
        return Err(AppError::new(
            "AI_COMPONENT_MANIFEST_INVALID",
            "组件清单平台或版本不受支持",
        ));
    }
    if manifest.components.is_empty() {
        return Err(AppError::new(
            "AI_COMPONENT_MANIFEST_INVALID",
            "组件清单不能为空",
        ));
    }
    let mut ids = HashSet::new();
    for release in &manifest.components {
        validate_release(release)?;
        if !ids.insert(release.id.clone()) {
            return Err(AppError::new(
                "AI_COMPONENT_MANIFEST_INVALID",
                "组件清单包含重复组件",
            ));
        }
    }
    Ok(())
}

fn validate_release(release: &ComponentRelease) -> Result<(), AppError> {
    if release.platform != AI_COMPONENT_PLATFORM {
        return Err(AppError::new(
            "AI_COMPONENT_UNSUPPORTED_PLATFORM",
            "当前系统不支持该媒体组件",
        ));
    }
    if !valid_id(&release.id) || release.version.is_empty() || release.version.len() > 64 {
        return Err(AppError::new(
            "AI_COMPONENT_MANIFEST_INVALID",
            "组件标识或版本无效",
        ));
    }
    if !valid_sha256(&release.sha256) {
        return Err(AppError::new(
            "AI_COMPONENT_MANIFEST_INVALID",
            "组件校验值无效",
        ));
    }
    if release.download_bytes == 0 || release.installed_bytes == 0 {
        return Err(AppError::new(
            "AI_COMPONENT_MANIFEST_INVALID",
            "组件大小无效",
        ));
    }
    let url = Url::parse(&release.url)
        .map_err(|_| AppError::new("AI_COMPONENT_MANIFEST_INVALID", "组件下载地址无效"))?;
    if !is_allowed_download_url(&url) {
        return Err(AppError::new(
            "AI_COMPONENT_MANIFEST_INVALID",
            "组件必须通过 HTTPS 下载",
        ));
    }
    validate_entrypoint(&release.entrypoint)?;
    if !release.parts.is_empty() {
        if release.parts.len() < 2 {
            return Err(AppError::new(
                "AI_COMPONENT_MANIFEST_INVALID",
                "组件分片清单无效",
            ));
        }
        let mut total = 0u64;
        for part in &release.parts {
            if part.bytes == 0 {
                return Err(AppError::new(
                    "AI_COMPONENT_MANIFEST_INVALID",
                    "组件分片大小无效",
                ));
            }
            let part_url = Url::parse(&part.url)
                .map_err(|_| AppError::new("AI_COMPONENT_MANIFEST_INVALID", "组件下载地址无效"))?;
            if !is_allowed_download_url(&part_url) {
                return Err(AppError::new(
                    "AI_COMPONENT_MANIFEST_INVALID",
                    "组件必须通过 HTTPS 下载",
                ));
            }
            total = total.checked_add(part.bytes).ok_or_else(|| {
                AppError::new("AI_COMPONENT_MANIFEST_INVALID", "组件分片大小无效")
            })?;
        }
        if total != release.download_bytes {
            return Err(AppError::new(
                "AI_COMPONENT_MANIFEST_INVALID",
                "组件分片大小无效",
            ));
        }
    }
    Ok(())
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn is_allowed_download_url(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" if cfg!(test) && url.host_str().is_some_and(is_loopback_host) => true,
        _ => false,
    }
}

fn is_allowed_proxy_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
        && url.host_str().is_some()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn is_allowed_mirror_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn component_download_client(proxy: Option<&Url>) -> Result<reqwest::blocking::Client, AppError> {
    let mut builder = reqwest::blocking::Client::builder()
        .connect_timeout(COMPONENT_DOWNLOAD_CONNECT_TIMEOUT)
        .timeout(COMPONENT_DOWNLOAD_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if is_safe_component_redirect(attempt.previous().last(), attempt.url()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }));
    if let Some(proxy) = proxy {
        let proxy = reqwest::Proxy::all(proxy.as_str()).map_err(|error| {
            AppError::with_cause(
                "AI_COMPONENT_NETWORK_INVALID",
                "代理地址无效",
                error.to_string(),
            )
        })?;
        builder = builder.proxy(proxy);
    }
    builder.build().map_err(|error| {
        AppError::with_cause(
            "AI_COMPONENT_DOWNLOAD_FAILED",
            "无法创建下载客户端",
            error.to_string(),
        )
    })
}

fn is_safe_component_redirect(previous: Option<&Url>, current: &Url) -> bool {
    previous.is_some_and(|url| url.scheme() == "https") && current.scheme() == "https"
}

fn valid_id(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'))
        && value.len() <= 64
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|ch| matches!(ch, b'0'..=b'9' | b'a'..=b'f'))
        && value.bytes().any(|ch| ch != b'0')
}

fn validate_entrypoint(value: &str) -> Result<(), AppError> {
    let path = Path::new(value);
    if value.is_empty() || value.contains('\0') || path.is_absolute() {
        return Err(AppError::new(
            "AI_COMPONENT_MANIFEST_INVALID",
            "组件入口路径无效",
        ));
    }
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let name = part.to_string_lossy();
                if name == "." || name == ".." || name.contains('\0') {
                    return Err(AppError::new(
                        "AI_COMPONENT_MANIFEST_INVALID",
                        "组件入口路径无效",
                    ));
                }
            }
            _ => {
                return Err(AppError::new(
                    "AI_COMPONENT_MANIFEST_INVALID",
                    "组件入口路径无效",
                ));
            }
        }
    }
    Ok(())
}

fn emit(progress: &mut dyn FnMut(ComponentProgress), id: &str, stage: &str, percent: f64) {
    progress(ComponentProgress {
        id: id.to_string(),
        stage: stage.to_string(),
        percent,
    });
}

#[cfg(test)]
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn unique_dir(parent: &Path, prefix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    parent.join(format!("{prefix}-{nonce}"))
}

fn load_installed(root: &Path) -> Result<InstalledStore, AppError> {
    let path = root.join(INSTALLED_STORE);
    if !path.exists() {
        return Ok(InstalledStore {
            version: 1,
            items: Vec::new(),
        });
    }
    let bytes = fs::read(&path).map_err(|error| {
        AppError::with_cause("AI_COMPONENT_IO", "无法读取组件安装记录", error.to_string())
    })?;
    serde_json::from_slice(&bytes).map_err(|_| AppError::new("AI_COMPONENT_IO", "组件安装记录损坏"))
}

fn persist_installed(root: &Path, store: &InstalledStore) -> Result<(), AppError> {
    let path = root.join(INSTALLED_STORE);
    let temp = root.join(format!("{INSTALLED_STORE}.tmp"));
    let bytes = serde_json::to_vec_pretty(store).map_err(|error| {
        AppError::with_cause(
            "AI_COMPONENT_IO",
            "无法序列化组件安装记录",
            error.to_string(),
        )
    })?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)
        .map_err(|error| {
            AppError::with_cause("AI_COMPONENT_IO", "无法写入组件安装记录", error.to_string())
        })?;
    let mut writer = BufWriter::new(file);
    writer.write_all(&bytes).map_err(|error| {
        AppError::with_cause("AI_COMPONENT_IO", "无法写入组件安装记录", error.to_string())
    })?;
    writer.flush().map_err(|error| {
        AppError::with_cause("AI_COMPONENT_IO", "无法写入组件安装记录", error.to_string())
    })?;
    writer.get_ref().sync_all().map_err(|error| {
        AppError::with_cause("AI_COMPONENT_IO", "无法同步组件安装记录", error.to_string())
    })?;
    drop(writer);
    fs::rename(&temp, &path).map_err(|error| {
        AppError::with_cause("AI_COMPONENT_IO", "无法保存组件安装记录", error.to_string())
    })?;
    Ok(())
}

fn stream_download(
    url: &str,
    part_path: &Path,
    expected_bytes: u64,
    id: &str,
    progress: &mut dyn FnMut(ComponentProgress),
    proxy: Option<&Url>,
) -> Result<String, AppError> {
    let parsed = Url::parse(url)
        .map_err(|_| AppError::new("AI_COMPONENT_DOWNLOAD_FAILED", "组件下载地址无效"))?;
    if !is_allowed_download_url(&parsed) {
        return Err(AppError::new(
            "AI_COMPONENT_DOWNLOAD_FAILED",
            "组件必须通过 HTTPS 下载",
        ));
    }
    let client = component_download_client(proxy)?;
    let mut response = client.get(url).send().map_err(|error| {
        AppError::with_cause(
            "AI_COMPONENT_DOWNLOAD_FAILED",
            "无法下载媒体组件",
            error.to_string(),
        )
    })?;
    if response.status().is_redirection() || !response.status().is_success() {
        return Err(AppError::new(
            "AI_COMPONENT_DOWNLOAD_FAILED",
            format!("媒体组件下载失败（HTTP {}）", response.status()),
        ));
    }
    if let Some(parent) = part_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            AppError::with_cause("AI_COMPONENT_IO", "无法创建组件下载目录", error.to_string())
        })?;
    }
    let mut file = File::create(part_path).map_err(|error| {
        AppError::with_cause("AI_COMPONENT_IO", "无法写入组件下载文件", error.to_string())
    })?;
    let total_bytes = response.content_length().unwrap_or(expected_bytes).max(1);
    let mut downloaded_bytes = 0u64;
    let mut last_percent = 20.0;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 256 * 1024];
    loop {
        let read = response.read(&mut buffer).map_err(|error| {
            AppError::with_cause(
                "AI_COMPONENT_DOWNLOAD_FAILED",
                "无法读取媒体组件",
                error.to_string(),
            )
        })?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(|error| {
            AppError::with_cause("AI_COMPONENT_IO", "无法写入组件下载文件", error.to_string())
        })?;
        hasher.update(&buffer[..read]);
        downloaded_bytes = downloaded_bytes.saturating_add(read as u64);
        let percent = 20.0 + (downloaded_bytes as f64 / total_bytes as f64).min(1.0) * 25.0;
        if percent - last_percent >= 1.0 || percent >= 45.0 {
            emit(progress, id, "downloading", percent);
            last_percent = percent;
        }
    }
    file.flush().map_err(|error| {
        AppError::with_cause("AI_COMPONENT_IO", "无法写入组件下载文件", error.to_string())
    })?;
    file.sync_all().map_err(|error| {
        AppError::with_cause("AI_COMPONENT_IO", "无法同步组件下载文件", error.to_string())
    })?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn stream_download_parts(
    parts: &[(String, u64)],
    part_path: &Path,
    expected_bytes: u64,
    id: &str,
    progress: &mut dyn FnMut(ComponentProgress),
    proxy: Option<&Url>,
) -> Result<String, AppError> {
    if parts.is_empty() {
        return Err(AppError::new(
            "AI_COMPONENT_DOWNLOAD_FAILED",
            "组件分片清单无效",
        ));
    }
    let client = component_download_client(proxy)?;
    if let Some(parent) = part_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            AppError::with_cause("AI_COMPONENT_IO", "无法创建组件下载目录", error.to_string())
        })?;
    }
    let mut file = File::create(part_path).map_err(|error| {
        AppError::with_cause("AI_COMPONENT_IO", "无法写入组件下载文件", error.to_string())
    })?;
    let total_bytes = expected_bytes.max(1);
    let mut downloaded_bytes = 0u64;
    let mut last_percent = 20.0;
    let mut hasher = Sha256::new();
    for (url, part_bytes) in parts {
        let parsed = Url::parse(url)
            .map_err(|_| AppError::new("AI_COMPONENT_DOWNLOAD_FAILED", "组件下载地址无效"))?;
        if !is_allowed_download_url(&parsed) {
            return Err(AppError::new(
                "AI_COMPONENT_DOWNLOAD_FAILED",
                "组件必须通过 HTTPS 下载",
            ));
        }
        let mut response = client.get(url).send().map_err(|error| {
            AppError::with_cause(
                "AI_COMPONENT_DOWNLOAD_FAILED",
                "无法下载媒体组件",
                error.to_string(),
            )
        })?;
        if response.status().is_redirection() || !response.status().is_success() {
            return Err(AppError::new(
                "AI_COMPONENT_DOWNLOAD_FAILED",
                format!("媒体组件下载失败（HTTP {}）", response.status()),
            ));
        }
        if let Some(content_length) = response.content_length() {
            if content_length != *part_bytes {
                return Err(AppError::new(
                    "AI_COMPONENT_DOWNLOAD_FAILED",
                    "媒体组件分片大小不匹配",
                ));
            }
        }
        let mut buffer = [0u8; 256 * 1024];
        let mut received = 0u64;
        loop {
            let read = response.read(&mut buffer).map_err(|error| {
                AppError::with_cause(
                    "AI_COMPONENT_DOWNLOAD_FAILED",
                    "无法读取媒体组件",
                    error.to_string(),
                )
            })?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read]).map_err(|error| {
                AppError::with_cause("AI_COMPONENT_IO", "无法写入组件下载文件", error.to_string())
            })?;
            hasher.update(&buffer[..read]);
            received = received.saturating_add(read as u64);
            downloaded_bytes = downloaded_bytes.saturating_add(read as u64);
            let percent = 20.0 + (downloaded_bytes as f64 / total_bytes as f64).min(1.0) * 25.0;
            if percent - last_percent >= 1.0 || percent >= 45.0 {
                emit(progress, id, "downloading", percent);
                last_percent = percent;
            }
        }
        if received != *part_bytes {
            return Err(AppError::new(
                "AI_COMPONENT_DOWNLOAD_FAILED",
                "媒体组件分片大小不匹配",
            ));
        }
    }
    if downloaded_bytes != expected_bytes {
        return Err(AppError::new(
            "AI_COMPONENT_DOWNLOAD_FAILED",
            "媒体组件分片大小不匹配",
        ));
    }
    file.flush().map_err(|error| {
        AppError::with_cause("AI_COMPONENT_IO", "无法写入组件下载文件", error.to_string())
    })?;
    file.sync_all().map_err(|error| {
        AppError::with_cause("AI_COMPONENT_IO", "无法同步组件下载文件", error.to_string())
    })?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn sha256_file(path: &Path) -> Result<String, AppError> {
    let mut file = File::open(path).map_err(|error| {
        AppError::with_cause("AI_COMPONENT_IO", "无法读取组件下载文件", error.to_string())
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            AppError::with_cause("AI_COMPONENT_IO", "无法读取组件下载文件", error.to_string())
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn publish_atomically(
    root: &Path,
    id: &str,
    version: &str,
    staging: &Path,
) -> Result<(), AppError> {
    let parent = root.join(id);
    fs::create_dir_all(&parent).map_err(|error| {
        AppError::with_cause("AI_COMPONENT_IO", "无法创建组件发布目录", error.to_string())
    })?;
    let published = parent.join(version);
    let incoming = unique_dir(&parent, &format!(".{version}-new"));
    fs::rename(staging, &incoming).map_err(|error| {
        AppError::with_cause("AI_COMPONENT_IO", "无法发布组件", error.to_string())
    })?;
    if published.exists() {
        let backup = unique_dir(&parent, &format!(".{version}-old"));
        fs::rename(&published, &backup).map_err(|error| {
            AppError::with_cause("AI_COMPONENT_IO", "无法替换组件目录", error.to_string())
        })?;
        if let Err(error) = fs::rename(&incoming, &published) {
            let _ = fs::rename(&backup, &published);
            return Err(AppError::with_cause(
                "AI_COMPONENT_IO",
                "无法发布组件",
                error.to_string(),
            ));
        }
        let _ = fs::remove_dir_all(&backup);
    } else if let Err(error) = fs::rename(&incoming, &published) {
        return Err(AppError::with_cause(
            "AI_COMPONENT_IO",
            "无法发布组件",
            error.to_string(),
        ));
    }
    Ok(())
}

fn archive_looks_like_zip(archive: &Path) -> bool {
    // Downloads are stored as `{id}.part`, so the local extension is not a
    // reliable archive type. Detect ZIP by magic to support Windows runtimes.
    let mut file = match File::open(archive) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut magic = [0u8; 4];
    match file.read_exact(&mut magic) {
        Ok(()) => matches!(&magic, b"PK\x03\x04" | b"PK\x05\x06" | b"PK\x07\x08"),
        Err(_) => false,
    }
}

fn extract_archive(archive: &Path, destination: &Path) -> Result<(), AppError> {
    if archive_looks_like_zip(archive) {
        return extract_zip(archive, destination);
    }
    let listing = background_command(tar_program())
        .args(["-tf", &archive.to_string_lossy()])
        .output()
        .map_err(|error| {
            AppError::with_cause(
                "AI_COMPONENT_EXTRACT_FAILED",
                "无法读取组件归档",
                error.to_string(),
            )
        })?;
    if !listing.status.success() {
        return Err(AppError::new(
            "AI_COMPONENT_EXTRACT_FAILED",
            "组件归档无法读取",
        ));
    }
    let names = String::from_utf8_lossy(&listing.stdout);
    for name in names.lines() {
        if name.is_empty() {
            continue;
        }
        if !archive_member_is_safe(name) {
            return Err(AppError::new(
                "AI_COMPONENT_EXTRACT_FAILED",
                "组件归档包含非法路径",
            ));
        }
    }
    let status = background_command(tar_program())
        .args([
            "-xf",
            &archive.to_string_lossy(),
            "-C",
            &destination.to_string_lossy(),
        ])
        .status()
        .map_err(|error| {
            AppError::with_cause(
                "AI_COMPONENT_EXTRACT_FAILED",
                "无法解压组件归档",
                error.to_string(),
            )
        })?;
    if !status.success() {
        return Err(AppError::new(
            "AI_COMPONENT_EXTRACT_FAILED",
            "组件归档解压失败",
        ));
    }
    contain_extracted_tree(destination)
}

fn contain_extracted_tree(destination: &Path) -> Result<(), AppError> {
    let canonical_root = fs::canonicalize(destination).map_err(|error| {
        AppError::with_cause(
            "AI_COMPONENT_EXTRACT_FAILED",
            "无法验证组件目录",
            error.to_string(),
        )
    })?;
    fn walk(root: &Path, current: &Path) -> Result<(), AppError> {
        let entries = fs::read_dir(current).map_err(|error| {
            AppError::with_cause(
                "AI_COMPONENT_EXTRACT_FAILED",
                "无法验证组件目录",
                error.to_string(),
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                AppError::with_cause(
                    "AI_COMPONENT_EXTRACT_FAILED",
                    "无法验证组件目录",
                    error.to_string(),
                )
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                AppError::with_cause(
                    "AI_COMPONENT_EXTRACT_FAILED",
                    "无法验证组件目录",
                    error.to_string(),
                )
            })?;
            let canonical = fs::canonicalize(&path).map_err(|_| {
                AppError::new("AI_COMPONENT_EXTRACT_FAILED", "组件归档包含非法路径")
            })?;
            if !canonical.starts_with(root) {
                return Err(AppError::new(
                    "AI_COMPONENT_EXTRACT_FAILED",
                    "组件归档包含非法路径",
                ));
            }
            if metadata.file_type().is_symlink() {
                let target = fs::read_link(&path).map_err(|_| {
                    AppError::new("AI_COMPONENT_EXTRACT_FAILED", "组件归档包含非法路径")
                })?;
                if target.is_absolute()
                    || target.components().any(|component| {
                        matches!(
                            component,
                            Component::ParentDir | Component::RootDir | Component::Prefix(_)
                        )
                    })
                {
                    return Err(AppError::new(
                        "AI_COMPONENT_EXTRACT_FAILED",
                        "组件归档包含非法路径",
                    ));
                }
                continue;
            }
            if metadata.is_dir() {
                walk(root, &path)?;
            }
        }
        Ok(())
    }
    walk(&canonical_root, destination)
}

fn verify_entrypoint(root: &Path, entrypoint: &Path) -> Result<(), AppError> {
    let canonical_root = fs::canonicalize(root).map_err(|error| {
        AppError::with_cause(
            "AI_COMPONENT_EXTRACT_FAILED",
            "无法验证组件目录",
            error.to_string(),
        )
    })?;
    let metadata = fs::symlink_metadata(entrypoint)
        .map_err(|_| AppError::new("AI_COMPONENT_EXTRACT_FAILED", "组件入口不存在"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() == 0 {
        return Err(AppError::new(
            "AI_COMPONENT_EXTRACT_FAILED",
            "组件入口不是有效的普通文件",
        ));
    }
    let canonical = fs::canonicalize(entrypoint)
        .map_err(|_| AppError::new("AI_COMPONENT_EXTRACT_FAILED", "无法验证组件入口"))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(AppError::new(
            "AI_COMPONENT_EXTRACT_FAILED",
            "组件入口越出安装目录",
        ));
    }
    Ok(())
}

fn archive_member_is_safe(name: &str) -> bool {
    let normalized = name.replace('\\', "/");
    !normalized.is_empty()
        && !normalized.starts_with('/')
        && !normalized.split('/').any(|part| part == "..")
}

#[cfg(windows)]
fn is_windows_executable(entrypoint: &Path) -> bool {
    entrypoint
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
}

fn zip_entry_is_symlink(entry: &zip::read::ZipFile<'_>) -> bool {
    entry
        .unix_mode()
        .is_some_and(|mode| mode & 0o170000 == 0o120000)
}

fn extract_zip(archive: &Path, destination: &Path) -> Result<(), AppError> {
    let file = File::open(archive).map_err(|error| {
        AppError::with_cause(
            "AI_COMPONENT_EXTRACT_FAILED",
            "无法读取组件归档",
            error.to_string(),
        )
    })?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|_| AppError::new("AI_COMPONENT_EXTRACT_FAILED", "组件归档无法读取"))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|_| AppError::new("AI_COMPONENT_EXTRACT_FAILED", "组件归档无法读取"))?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(AppError::new(
                "AI_COMPONENT_EXTRACT_FAILED",
                "组件归档包含非法路径",
            ));
        };
        if !archive_member_is_safe(&relative.to_string_lossy()) {
            return Err(AppError::new(
                "AI_COMPONENT_EXTRACT_FAILED",
                "组件归档包含非法路径",
            ));
        }
        let out_path = destination.join(relative);
        if !out_path.starts_with(destination) {
            return Err(AppError::new(
                "AI_COMPONENT_EXTRACT_FAILED",
                "组件归档包含非法路径",
            ));
        }
        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|error| {
                AppError::with_cause(
                    "AI_COMPONENT_EXTRACT_FAILED",
                    "无法解压组件归档",
                    error.to_string(),
                )
            })?;
            continue;
        }
        if zip_entry_is_symlink(&entry) {
            return Err(AppError::new(
                "AI_COMPONENT_EXTRACT_FAILED",
                "组件归档包含非法路径",
            ));
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                AppError::with_cause(
                    "AI_COMPONENT_EXTRACT_FAILED",
                    "无法解压组件归档",
                    error.to_string(),
                )
            })?;
        }
        let mut output = File::create(&out_path).map_err(|error| {
            AppError::with_cause(
                "AI_COMPONENT_EXTRACT_FAILED",
                "无法解压组件归档",
                error.to_string(),
            )
        })?;
        io::copy(&mut entry, &mut output).map_err(|error| {
            AppError::with_cause(
                "AI_COMPONENT_EXTRACT_FAILED",
                "无法解压组件归档",
                error.to_string(),
            )
        })?;
    }
    contain_extracted_tree(destination)
}

fn should_run_self_test(entrypoint: &Path) -> Result<bool, AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(entrypoint)
            .map_err(|_| AppError::new("AI_COMPONENT_SELF_TEST_FAILED", "无法读取组件入口"))?;
        Ok(metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(windows)]
    {
        Ok(is_windows_executable(entrypoint))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = entrypoint;
        Ok(false)
    }
}

fn run_self_test(entrypoint: &Path) -> Result<(), AppError> {
    if !should_run_self_test(entrypoint)? {
        return Ok(());
    }
    let output = background_command(entrypoint)
        .arg("--self-test")
        .stdin(Stdio::null())
        .output()
        .map_err(|_| AppError::new("AI_COMPONENT_SELF_TEST_FAILED", "组件自检无法启动"))?;
    if !output.status.success() {
        return Err(AppError::new(
            "AI_COMPONENT_SELF_TEST_FAILED",
            "组件自检失败",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod archive_kind_tests {
    use super::*;
    use std::fs;

    #[test]
    fn part_files_are_detected_by_zip_magic_not_extension() {
        // Production mutation caught: inspecting only the `.zip` extension after
        // the installer saved the archive as `{id}.part`.
        let root = std::env::temp_dir().join(format!(
            "hongguo-zip-magic-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let part = root.join("runtime.part");
        fs::write(&part, b"PK\x03\x04xxxx").unwrap();
        assert!(archive_looks_like_zip(&part));
        fs::write(&part, b"\x1f\x8bgzip-header").unwrap();
        assert!(!archive_looks_like_zip(&part));
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(unix)]
fn tar_program() -> &'static str {
    "/usr/bin/tar"
}

#[cfg(windows)]
fn tar_program() -> &'static str {
    "tar.exe"
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        collections::HashMap,
        fs,
        io::{Read, Write},
        net::TcpListener,
        os::unix::fs::PermissionsExt,
        process::Command,
        sync::{
            atomic::{AtomicU64, AtomicUsize, Ordering},
            mpsc, Arc, Mutex,
        },
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let unique = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "hongguo-ai-components-{}-{}-{unique}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn manager(&self, sha: &str) -> ComponentManager {
            self.manager_with_url(sha, "https://example.invalid/runtime.tar")
        }

        fn manager_with_url(&self, sha: &str, url: &str) -> ComponentManager {
            self.manager_with(sha, url, "hongguo-ai", 32)
        }

        fn manager_with(
            &self,
            sha: &str,
            url: &str,
            entrypoint: &str,
            size: u64,
        ) -> ComponentManager {
            let manifest = json!({
                "version": 1,
                "platform": "aarch64-apple-darwin",
                "components": [{
                    "id": "runtime",
                    "version": "1",
                    "platform": "aarch64-apple-darwin",
                    "url": url,
                    "sha256": sha,
                    "downloadBytes": size,
                    "installedBytes": size,
                    "entrypoint": entrypoint
                }]
            });
            ComponentManager::from_manifest_json(&self.root, &manifest.to_string()).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn archive_with_entrypoint(dir: &Path, script: &str) -> (Vec<u8>, String) {
        archive_named(dir, "hongguo-ai", script, 0o755)
    }

    fn archive_named(dir: &Path, name: &str, script: &str, mode: u32) -> (Vec<u8>, String) {
        let unique = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let source = dir.join(format!("src-{unique}"));
        fs::create_dir_all(&source).unwrap();
        let entry = source.join(name);
        fs::write(&entry, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&entry, fs::Permissions::from_mode(mode)).unwrap();
        let archive = dir.join(format!("runtime-{unique}.tar"));
        Command::new("/usr/bin/tar")
            .current_dir(&source)
            .args(["-cf", &archive.to_string_lossy(), name])
            .status()
            .unwrap();
        let bytes = fs::read(&archive).unwrap();
        let digest = sha256_hex(&bytes);
        (bytes, digest)
    }

    fn seed_previous_version(root: &Path) {
        let published = root.join("runtime").join("1");
        fs::create_dir_all(&published).unwrap();
        fs::write(published.join("hongguo-ai"), b"previous-version").unwrap();
        persist_installed(
            root,
            &InstalledStore {
                version: 1,
                items: vec![InstalledRecord {
                    id: "runtime".into(),
                    version: "1".into(),
                }],
            },
        )
        .unwrap();
    }

    struct MockHttp {
        url: String,
        requests: Arc<AtomicUsize>,
        max_in_flight: Arc<AtomicUsize>,
        shutdown: Arc<Mutex<bool>>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl MockHttp {
        fn serve(body: Vec<u8>) -> Self {
            Self::serve_with(body, false)
        }

        fn serve_with(body: Vec<u8>, slow: bool) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("loopback mock should bind");
            listener
                .set_nonblocking(true)
                .expect("loopback mock should be nonblocking");
            let port = listener.local_addr().unwrap().port();
            let requests = Arc::new(AtomicUsize::new(0));
            let in_flight = Arc::new(AtomicUsize::new(0));
            let max_in_flight = Arc::new(AtomicUsize::new(0));
            let shutdown = Arc::new(Mutex::new(false));
            let thread = thread::spawn({
                let requests = requests.clone();
                let in_flight = in_flight.clone();
                let max_in_flight = max_in_flight.clone();
                let shutdown = shutdown.clone();
                move || loop {
                    if *shutdown.lock().unwrap() {
                        break;
                    }
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            requests.fetch_add(1, Ordering::SeqCst);
                            let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                            max_in_flight.fetch_max(current, Ordering::SeqCst);
                            let mut buf = [0u8; 1024];
                            let _ = stream.read(&mut buf);
                            if slow {
                                thread::sleep(Duration::from_millis(250));
                            }
                            let header = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                    body.len()
                                );
                            let _ = stream.write_all(header.as_bytes());
                            let _ = stream.write_all(&body);
                            let _ = stream.flush();
                            in_flight.fetch_sub(1, Ordering::SeqCst);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                url: format!("http://127.0.0.1:{port}/runtime.tar"),
                requests,
                max_in_flight,
                shutdown,
                thread: Some(thread),
            }
        }

        fn serve_routes(routes: Vec<(String, Vec<u8>)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("loopback mock should bind");
            listener
                .set_nonblocking(true)
                .expect("loopback mock should be nonblocking");
            let port = listener.local_addr().unwrap().port();
            let requests = Arc::new(AtomicUsize::new(0));
            let in_flight = Arc::new(AtomicUsize::new(0));
            let max_in_flight = Arc::new(AtomicUsize::new(0));
            let shutdown = Arc::new(Mutex::new(false));
            let bodies: HashMap<String, Vec<u8>> = routes.into_iter().collect();
            let thread = thread::spawn({
                let requests = requests.clone();
                let in_flight = in_flight.clone();
                let max_in_flight = max_in_flight.clone();
                let shutdown = shutdown.clone();
                move || loop {
                    if *shutdown.lock().unwrap() {
                        break;
                    }
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            requests.fetch_add(1, Ordering::SeqCst);
                            let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                            max_in_flight.fetch_max(current, Ordering::SeqCst);
                            // TCP reads may split the request line under parallel test load.
                            // Read bounded, complete headers before selecting a mock route.
                            use std::io::{BufRead, BufReader};
                            stream
                                .set_read_timeout(Some(Duration::from_secs(5)))
                                .unwrap();
                            let mut request = String::new();
                            let mut reader = BufReader::new((&mut stream).take(16 * 1024));
                            loop {
                                let mut line = String::new();
                                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                                    break;
                                }
                                let end = line == "\r\n" || line == "\n";
                                request.push_str(&line);
                                if end {
                                    break;
                                }
                            }
                            drop(reader);
                            let path = request
                                .lines()
                                .next()
                                .and_then(|line| line.split_whitespace().nth(1))
                                .unwrap_or("/")
                                .split('?')
                                .next()
                                .unwrap_or("/")
                                .to_string();
                            if let Some(body) = bodies.get(&path) {
                                let header = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                    body.len()
                                );
                                let _ = stream.write_all(header.as_bytes());
                                let _ = stream.write_all(body);
                            } else {
                                let header = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                                let _ = stream.write_all(header.as_bytes());
                            }
                            let _ = stream.flush();
                            in_flight.fetch_sub(1, Ordering::SeqCst);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                url: format!("http://127.0.0.1:{port}"),
                requests,
                max_in_flight,
                shutdown,
                thread: Some(thread),
            }
        }

        fn request_count(&self) -> usize {
            self.requests.load(Ordering::SeqCst)
        }

        fn max_in_flight(&self) -> usize {
            self.max_in_flight.load(Ordering::SeqCst)
        }
    }

    impl Drop for MockHttp {
        fn drop(&mut self) {
            *self.shutdown.lock().unwrap() = true;
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    #[test]
    fn checksum_failure_never_replaces_the_current_component() {
        // Production mutation caught: publishing a corrupt payload over an already installed version.
        let fixture = Fixture::new();
        let (good, sha) = archive_with_entrypoint(&fixture.root, "exit 0");
        let manager = fixture.manager(&sha);
        manager
            .install_bytes("runtime", &good, &sha, |_| {})
            .unwrap();
        assert_eq!(manager.installed_version("runtime").as_deref(), Some("1"));

        let error = manager
            .install_bytes("runtime", b"corrupt", "00", |_| {})
            .unwrap_err();
        assert_eq!(error.code, "AI_COMPONENT_CHECKSUM_FAILED");
        assert_eq!(manager.installed_version("runtime").as_deref(), Some("1"));
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains(fixture.root.to_string_lossy().as_ref()));
    }

    #[test]
    fn running_component_cannot_be_removed() {
        // Production mutation caught: deleting a component while a media job still references it.
        let fixture = Fixture::new();
        let (good, sha) = archive_with_entrypoint(&fixture.root, "exit 0");
        let manager = fixture.manager(&sha);
        manager
            .install_bytes("runtime", &good, &sha, |_| {})
            .unwrap();
        manager.mark_in_use("runtime", true);
        assert_eq!(
            manager.remove("runtime").unwrap_err().code,
            "AI_COMPONENT_IN_USE"
        );
        assert_eq!(manager.installed_version("runtime").as_deref(), Some("1"));
    }

    #[test]
    fn shared_component_stays_in_use_until_last_task_finishes() {
        let fixture = Fixture::new();
        let (good, sha) = archive_with_entrypoint(&fixture.root, "exit 0");
        let manager = fixture.manager(&sha);
        manager
            .install_bytes("runtime", &good, &sha, |_| {})
            .unwrap();
        manager.mark_in_use("runtime", true);
        manager.mark_in_use("runtime", true);
        manager.mark_in_use("runtime", false);
        assert!(
            manager
                .status()
                .iter()
                .find(|item| item.id == "runtime")
                .unwrap()
                .in_use
        );
        assert_eq!(
            manager.remove("runtime").unwrap_err().code,
            "AI_COMPONENT_IN_USE"
        );
        manager.mark_in_use("runtime", false);
        manager.remove("runtime").unwrap();
    }

    #[test]
    fn unsupported_platform_and_invalid_manifest_are_rejected() {
        // Production mutation caught: accepting a non-ARM64 platform or unknown JSON fields.
        let error =
            parse_manifest(r#"{"version":1,"platform":"x86_64-apple-darwin","components":[]}"#)
                .unwrap_err();
        assert_eq!(error.code, "AI_COMPONENT_MANIFEST_INVALID");
        let error = parse_manifest(r#"{"version":1,"platform":"aarch64-apple-darwin","components":[{"id":"runtime","version":"1","platform":"aarch64-apple-darwin","url":"https://example.invalid/a","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","downloadBytes":1,"installedBytes":1,"entrypoint":"hongguo-ai","extra":true}]}"#).unwrap_err();
        assert_eq!(error.code, "AI_COMPONENT_MANIFEST_INVALID");
    }

    #[test]
    fn http_url_and_uppercase_sha_are_rejected() {
        // Production mutation caught: allowing non-HTTPS URLs or non-canonical checksums.
        let json = json!({
            "version": 1,
            "platform": "aarch64-apple-darwin",
            "components": [{
                "id": "runtime",
                "version": "1",
                "platform": "aarch64-apple-darwin",
                "url": "http://example.invalid/runtime.tar",
                "sha256": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                "downloadBytes": 1,
                "installedBytes": 1,
                "entrypoint": "hongguo-ai"
            }]
        });
        assert_eq!(
            parse_manifest(&json.to_string()).unwrap_err().code,
            "AI_COMPONENT_MANIFEST_INVALID"
        );
    }

    #[test]
    fn insufficient_disk_is_rejected_before_install() {
        // Production mutation caught: starting an install without a free-space precheck.
        let fixture = Fixture::new();
        let (good, sha) = archive_with_entrypoint(&fixture.root, "exit 0");
        let manager = fixture.manager(&sha);
        manager.set_min_free_bytes(Some(1));
        assert_eq!(
            manager
                .install_bytes("runtime", &good, &sha, |_| {})
                .unwrap_err()
                .code,
            "AI_COMPONENT_INSUFFICIENT_DISK"
        );
        assert!(manager.installed_version("runtime").is_none());
    }

    #[test]
    fn archive_path_traversal_is_rejected() {
        // Production mutation caught: extracting archive members that escape the staging directory.
        let fixture = Fixture::new();
        let source = fixture.root.join("evil-src");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("hongguo-ai"), b"#!/bin/sh\nexit 0\n").unwrap();
        let archive = fixture.root.join("evil.tar");
        Command::new("/usr/bin/tar")
            .current_dir(&source)
            .args(["-cf", &archive.to_string_lossy(), "hongguo-ai"])
            .status()
            .unwrap();
        // Rebuild a tar that includes a traversal name by copying bytes is hard; use tar with a crafted member via pax.
        let traversal = fixture.root.join("traversal.tar");
        let status = Command::new("python3")
            .args([
                "-c",
                &format!(
                    "import tarfile; tar=tarfile.open(r'{path}', 'w'); info=tarfile.TarInfo('../escape'); info.size=4; tar.addfile(info, __import__('io').BytesIO(b'data')); tar.close()",
                    path = traversal.display()
                ),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let bytes = fs::read(&traversal).unwrap();
        let sha = sha256_hex(&bytes);
        let manager = fixture.manager(&sha);
        let error = manager
            .install_bytes("runtime", &bytes, &sha, |_| {})
            .unwrap_err();
        assert_eq!(error.code, "AI_COMPONENT_EXTRACT_FAILED");
        assert!(manager.installed_version("runtime").is_none());
    }

    #[test]
    fn failed_self_test_does_not_publish() {
        // Production mutation caught: marking a component installed after a failing --self-test.
        let fixture = Fixture::new();
        let (bad, sha) = archive_with_entrypoint(&fixture.root, "exit 7");
        let manager = fixture.manager(&sha);
        assert_eq!(
            manager
                .install_bytes("runtime", &bad, &sha, |_| {})
                .unwrap_err()
                .code,
            "AI_COMPONENT_SELF_TEST_FAILED"
        );
        assert!(manager.installed_version("runtime").is_none());
        assert!(!fixture.root.join("runtime/1").exists());
    }

    #[test]
    fn zip_part_file_without_zip_extension_extracts() {
        // Production mutation caught: saving downloads as `{id}.part` and then
        // only treating files whose local extension is `.zip` as zip archives.
        let fixture = Fixture::new();
        let source = fixture.root.join("zip-src/hongguo-ai-worker");
        fs::create_dir_all(&source).unwrap();
        let entry = source.join("hongguo-ai-worker");
        fs::write(&entry, b"#!/bin/sh\nexit 0\n").unwrap();
        let archive = fixture.root.join("runtime.zip");
        let status = Command::new("python3")
            .args([
                "-c",
                &format!(
                    "import zipfile; z=zipfile.ZipFile(r'{path}', 'w'); z.write(r'{entry}', 'hongguo-ai-worker/hongguo-ai-worker'); z.close()",
                    path = archive.display(),
                    entry = entry.display(),
                ),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let part = fixture.root.join("runtime.part");
        fs::copy(&archive, &part).unwrap();
        assert!(archive_looks_like_zip(&part));
        let destination = fixture.root.join("extracted");
        fs::create_dir_all(&destination).unwrap();
        extract_archive(&part, &destination).unwrap();
        assert!(destination
            .join("hongguo-ai-worker/hongguo-ai-worker")
            .is_file());
    }

    #[test]
    fn zip_runtime_archive_installs_without_using_tar() {
        // Production mutation caught: Windows runtime zips failing because extract
        // always shells out to tar and cannot read Compress-Archive members.
        let fixture = Fixture::new();
        let source = fixture.root.join("zip-src/hongguo-ai-worker");
        fs::create_dir_all(&source).unwrap();
        let entry = source.join("hongguo-ai-worker");
        fs::write(&entry, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&entry, fs::Permissions::from_mode(0o755)).unwrap();
        let archive = fixture.root.join("runtime.zip");
        let status = Command::new("python3")
            .args([
                "-c",
                &format!(
                    "import zipfile; z=zipfile.ZipFile(r'{path}', 'w'); z.write(r'{entry}', 'hongguo-ai-worker/hongguo-ai-worker'); z.close()",
                    path = archive.display(),
                    entry = entry.display(),
                ),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let bytes = fs::read(&archive).unwrap();
        let sha = sha256_hex(&bytes);
        let manager = fixture.manager_with(
            &sha,
            "https://example.invalid/runtime.zip",
            "hongguo-ai-worker/hongguo-ai-worker",
            bytes.len() as u64,
        );
        let status = manager
            .install_bytes("runtime", &bytes, &sha, |_| {})
            .unwrap();
        assert!(status.installed);
        assert!(fixture
            .root
            .join("runtime/1/hongguo-ai-worker/hongguo-ai-worker")
            .is_file());
    }

    #[test]
    fn non_executable_model_entrypoint_skips_self_test() {
        // Production mutation caught: executing model weights/YAML with --self-test.
        let fixture = Fixture::new();
        let source = fixture.root.join("model-src");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("htdemucs.yaml"), b"name: htdemucs\n").unwrap();
        let archive = fixture.root.join("model.tar");
        assert!(Command::new("/usr/bin/tar")
            .current_dir(&source)
            .args(["-cf", &archive.to_string_lossy(), "htdemucs.yaml"])
            .status()
            .unwrap()
            .success());
        let bytes = fs::read(&archive).unwrap();
        let sha = sha256_hex(&bytes);
        let manager = fixture.manager_with(
            &sha,
            "https://example.invalid/model.tar",
            "htdemucs.yaml",
            bytes.len() as u64,
        );
        let status = manager
            .install_bytes("runtime", &bytes, &sha, |_| {})
            .unwrap();
        assert!(status.installed);
    }

    #[test]
    fn successful_install_is_atomic_and_reports_progress_stages() {
        // Production mutation caught: skipping installed.json publish or reporting success without a self-test.
        let fixture = Fixture::new();
        let (good, sha) = archive_with_entrypoint(&fixture.root, "exit 0");
        let manager = fixture.manager(&sha);
        let mut stages = Vec::new();
        let status = manager
            .install_bytes("runtime", &good, &sha, |progress| {
                stages.push(progress.stage)
            })
            .unwrap();
        assert!(status.installed);
        assert_eq!(status.installed_version.as_deref(), Some("1"));
        assert!(fixture.root.join("installed.json").is_file());
        assert!(fixture.root.join("runtime/1/hongguo-ai").is_file());
        for required in [
            "checking",
            "downloading",
            "verifying",
            "extracting",
            "selfTesting",
            "installed",
        ] {
            assert!(
                stages.iter().any(|stage| stage == required),
                "missing {required}"
            );
        }
    }

    #[test]
    fn concurrent_install_is_rejected() {
        // Production mutation caught: allowing two installs to share the same download/publish path.
        let fixture = Fixture::new();
        let (good, sha) = archive_with_entrypoint(&fixture.root, "exit 0");
        let manager = fixture.manager(&sha);
        manager.force_installing();
        assert_eq!(
            manager
                .install_bytes("runtime", &good, &sha, |_| {})
                .unwrap_err()
                .code,
            "AI_COMPONENT_INSTALL_RUNNING"
        );
    }

    #[test]
    fn render_script_exits_nonzero_without_required_env() {
        // Production mutation caught: emitting a blank/zero checksum manifest when release URLs are missing.
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/render-ai-component-manifest.sh");
        let status = Command::new("/bin/bash")
            .arg(&script)
            .env_remove("HONGGUO_AI_RUNTIME_URL")
            .status()
            .unwrap();
        assert!(!status.success());
    }

    #[test]
    fn concurrent_install_on_the_command_path_is_rejected() {
        // Production mutation caught: two install() calls both downloading before the single-install lock.
        let fixture = Fixture::new();
        let (good, sha) = archive_with_entrypoint(&fixture.root, "exit 0");
        let server = MockHttp::serve_with(good, true);
        let manager = Arc::new(fixture.manager_with_url(&sha, &server.url));
        let (started_tx, started_rx) = mpsc::channel();
        let first = {
            let manager = manager.clone();
            thread::spawn(move || {
                started_tx.send(()).unwrap();
                manager.install("runtime", |_| {})
            })
        };
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        thread::sleep(Duration::from_millis(40));
        let second = manager.install("runtime", |_| {}).unwrap_err();
        assert_eq!(second.code, "AI_COMPONENT_INSTALL_RUNNING");
        let _ = first.join().unwrap();
        assert!(
            server.max_in_flight() <= 1,
            "mock saw overlapping downloads: {}",
            server.max_in_flight()
        );
        assert!(server.request_count() >= 1);
    }

    #[test]
    fn insufficient_disk_on_the_command_path_does_not_touch_the_network() {
        // Production mutation caught: install() downloading before the free-space check.
        let fixture = Fixture::new();
        let (good, sha) = archive_with_entrypoint(&fixture.root, "exit 0");
        let server = MockHttp::serve(good);
        let manager = fixture.manager_with_url(&sha, &server.url);
        manager.set_min_free_bytes(Some(0));
        let mut stages = Vec::new();
        let error = manager
            .install("runtime", |progress| stages.push(progress.stage))
            .unwrap_err();
        assert_eq!(error.code, "AI_COMPONENT_INSUFFICIENT_DISK");
        assert_eq!(server.request_count(), 0);
        assert!(stages.iter().any(|stage| stage == "checking"));
        assert!(stages.iter().any(|stage| stage == "failed"));
    }

    #[test]
    fn leftover_part_file_is_never_marked_installed() {
        // Production mutation caught: treating a leftover .downloads/<id>.part as a completed install.
        let fixture = Fixture::new();
        let (good, sha) = archive_with_entrypoint(&fixture.root, "exit 0");
        seed_previous_version(&fixture.root);
        let server = MockHttp::serve(b"truncated-part".to_vec());
        let manager = fixture.manager_with_url(&sha, &server.url);
        fs::write(
            fixture.root.join(".downloads").join("runtime.part"),
            b"leftover",
        )
        .unwrap();
        let error = manager.install("runtime", |_| {}).unwrap_err();
        assert_eq!(error.code, "AI_COMPONENT_CHECKSUM_FAILED");
        assert_eq!(manager.installed_version("runtime").as_deref(), Some("1"));
        assert_eq!(manager.status()[0].installed_version.as_deref(), Some("1"));
        assert_eq!(
            fs::read(fixture.root.join("runtime/1/hongguo-ai")).unwrap(),
            b"previous-version"
        );
        let _ = good;
    }

    #[test]
    fn checksum_failure_on_the_command_path_never_replaces_current_version() {
        // Production mutation caught: install() replacing the published directory after a checksum miss.
        let fixture = Fixture::new();
        let (good, sha) = archive_with_entrypoint(&fixture.root, "exit 0");
        seed_previous_version(&fixture.root);
        let server = MockHttp::serve(b"not-the-archive".to_vec());
        let manager = fixture.manager_with_url(&sha, &server.url);
        let mut stages = Vec::new();
        let error = manager
            .install("runtime", |progress| stages.push(progress.stage))
            .unwrap_err();
        assert_eq!(error.code, "AI_COMPONENT_CHECKSUM_FAILED");
        assert_eq!(manager.installed_version("runtime").as_deref(), Some("1"));
        assert_eq!(
            fs::read(fixture.root.join("runtime/1/hongguo-ai")).unwrap(),
            b"previous-version"
        );
        assert!(stages.iter().any(|stage| stage == "failed"));
        let serialized = serde_json::to_string(&error).unwrap();
        assert!(!serialized.contains(fixture.root.to_string_lossy().as_ref()));
        let _ = good;
    }

    #[test]
    fn path_traversal_archive_on_the_command_path_leaves_previous_version() {
        // Production mutation caught: extracting archive members that escape staging, then publishing.
        let fixture = Fixture::new();
        seed_previous_version(&fixture.root);
        let traversal = fixture.root.join("traversal.tar");
        let status = Command::new("python3")
            .args([
                "-c",
                &format!(
                    "import tarfile, io; tar=tarfile.open(r'{path}', 'w'); info=tarfile.TarInfo('../escape'); info.size=4; tar.addfile(info, io.BytesIO(b'data')); tar.close()",
                    path = traversal.display()
                ),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let bytes = fs::read(&traversal).unwrap();
        let sha = sha256_hex(&bytes);
        let server = MockHttp::serve(bytes);
        let manager = fixture.manager_with_url(&sha, &server.url);
        let error = manager.install("runtime", |_| {}).unwrap_err();
        assert_eq!(error.code, "AI_COMPONENT_EXTRACT_FAILED");
        assert_eq!(manager.installed_version("runtime").as_deref(), Some("1"));
        assert_eq!(
            fs::read(fixture.root.join("runtime/1/hongguo-ai")).unwrap(),
            b"previous-version"
        );
        assert!(!fixture.root.join("escape").exists());
    }

    #[test]
    fn symlink_escape_archive_on_the_command_path_is_rejected() {
        // Production mutation caught: list-only tar -tf then unrestricted tar -xf of a symlink member.
        let fixture = Fixture::new();
        seed_previous_version(&fixture.root);
        let outside = fixture.root.join("outside-secret");
        fs::write(&outside, b"secret").unwrap();
        let archive = fixture.root.join("symlink.tar");
        let status = Command::new("python3")
            .args([
                "-c",
                &format!(
                    "import tarfile; tar=tarfile.open(r'{path}', 'w'); info=tarfile.TarInfo('hongguo-ai'); info.type=tarfile.SYMTYPE; info.linkname=r'{target}'; tar.addfile(info); tar.close()",
                    path = archive.display(),
                    target = outside.display()
                ),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let bytes = fs::read(&archive).unwrap();
        let sha = sha256_hex(&bytes);
        let server = MockHttp::serve(bytes);
        let manager = fixture.manager_with_url(&sha, &server.url);
        let error = manager.install("runtime", |_| {}).unwrap_err();
        assert_eq!(error.code, "AI_COMPONENT_EXTRACT_FAILED");
        assert_eq!(manager.installed_version("runtime").as_deref(), Some("1"));
        assert_eq!(
            fs::read(fixture.root.join("runtime/1/hongguo-ai")).unwrap(),
            b"previous-version"
        );
        assert!(fs::read_dir(fixture.root.join(".downloads"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("runtime-stage-")));
    }

    #[test]
    fn contained_relative_symlink_archive_installs_successfully() {
        // Production mutation caught: rejecting PyInstaller's relative aliases even
        // when both the link and canonical target remain inside the component root.
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let source = fixture.root.join("contained-symlink-src");
        fs::create_dir_all(source.join("internal/lib")).unwrap();
        fs::write(source.join("hongguo-ai"), b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(source.join("hongguo-ai"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(source.join("internal/lib/real.dylib"), b"library").unwrap();
        symlink("lib/real.dylib", source.join("internal/alias.dylib")).unwrap();
        let archive = fixture.root.join("contained-symlink.tar");
        assert!(Command::new("/usr/bin/tar")
            .current_dir(&source)
            .args(["-cf", &archive.to_string_lossy(), "."])
            .status()
            .unwrap()
            .success());
        let bytes = fs::read(archive).unwrap();
        let sha = sha256_hex(&bytes);
        let manager = fixture.manager(&sha);

        let status = manager
            .install_bytes("runtime", &bytes, &sha, |_| {})
            .unwrap();

        assert!(status.installed);
        let alias = fixture.root.join("runtime/1/internal/alias.dylib");
        assert!(fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(alias).unwrap(), b"library");
    }

    #[test]
    fn verified_partial_archive_is_reused_without_downloading_again() {
        // Production mutation caught: truncating a complete verified .part file
        // after a post-download failure and forcing the user to download it again.
        let fixture = Fixture::new();
        let (good, sha) = archive_with_entrypoint(&fixture.root, "exit 0");
        let server = MockHttp::serve(good.clone());
        let manager = fixture.manager_with(&sha, &server.url, "hongguo-ai", good.len() as u64);
        fs::write(fixture.root.join(".downloads/runtime.part"), &good).unwrap();

        let status = manager.install("runtime", |_| {}).unwrap();

        assert!(status.installed);
        assert_eq!(server.request_count(), 0);
    }

    #[test]
    fn multipart_release_installs_by_concatenating_http_parts() {
        // Production mutation caught: downloading only release.url when GitHub
        // requires the Windows runtime zip to be split under the 2GB asset limit.
        let fixture = Fixture::new();
        let (good, sha) = archive_with_entrypoint(&fixture.root, "exit 0");
        assert!(good.len() > 4, "archive should be large enough to split");
        let split_at = good.len() / 2;
        let first = good[..split_at].to_vec();
        let second = good[split_at..].to_vec();
        let server = MockHttp::serve_routes(vec![
            ("/runtime.tar.001".into(), first.clone()),
            ("/runtime.tar.002".into(), second.clone()),
        ]);
        let manifest = json!({
            "version": 1,
            "platform": "aarch64-apple-darwin",
            "components": [{
                "id": "runtime",
                "version": "1",
                "platform": "aarch64-apple-darwin",
                "url": format!("{}/runtime.tar", server.url),
                "sha256": sha,
                "downloadBytes": good.len() as u64,
                "installedBytes": good.len() as u64,
                "entrypoint": "hongguo-ai",
                "parts": [
                    {"url": format!("{}/runtime.tar.001", server.url), "bytes": first.len() as u64},
                    {"url": format!("{}/runtime.tar.002", server.url), "bytes": second.len() as u64}
                ]
            }]
        });
        let manager =
            ComponentManager::from_manifest_json(&fixture.root, &manifest.to_string()).unwrap();
        let status = manager.install("runtime", |_| {}).unwrap();
        assert!(status.installed);
        assert_eq!(server.request_count(), 2);
        assert_eq!(
            fs::read(fixture.root.join("runtime/1/hongguo-ai")).unwrap()[..2],
            b"#!"[..]
        );
    }

    #[test]
    fn multipart_bytes_must_sum_to_download_bytes() {
        // Production mutation caught: accepting a split manifest whose parts
        // do not reconstruct the advertised complete archive size.
        let json = json!({
            "version": 1,
            "platform": "aarch64-apple-darwin",
            "components": [{
                "id": "runtime",
                "version": "1",
                "platform": "aarch64-apple-darwin",
                "url": "https://example.invalid/runtime.tar",
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "downloadBytes": 10,
                "installedBytes": 10,
                "entrypoint": "hongguo-ai",
                "parts": [
                    {"url": "https://example.invalid/runtime.tar.001", "bytes": 6},
                    {"url": "https://example.invalid/runtime.tar.002", "bytes": 5}
                ]
            }]
        });
        assert_eq!(
            parse_manifest(&json.to_string()).unwrap_err().code,
            "AI_COMPONENT_MANIFEST_INVALID"
        );
    }

    #[test]
    fn successful_install_on_the_command_path_sets_installed_path_and_clears_part() {
        // Production mutation caught: deleting the current version before the new directory is in place, or leaving .part behind.
        let fixture = Fixture::new();
        seed_previous_version(&fixture.root);
        let (good, sha) = archive_with_entrypoint(&fixture.root, "exit 0");
        let server = MockHttp::serve(good);
        let manager = fixture.manager_with_url(&sha, &server.url);
        let mut stages = Vec::new();
        let status = manager
            .install("runtime", |progress| stages.push(progress.stage))
            .unwrap();
        assert!(status.installed);
        let installed_path = status.installed_path.expect("installedPath should be Some");
        assert!(
            installed_path.ends_with("runtime/1"),
            "installedPath={installed_path}"
        );
        assert!(!fixture
            .root
            .join(".downloads")
            .join("runtime.part")
            .exists());
        assert_eq!(
            fs::read(fixture.root.join("runtime/1/hongguo-ai")).unwrap()[..2],
            b"#!"[..]
        );
        for required in [
            "checking",
            "downloading",
            "verifying",
            "extracting",
            "selfTesting",
            "installed",
        ] {
            assert!(
                stages.iter().any(|stage| stage == required),
                "missing {required}"
            );
        }
    }

    #[test]
    fn invalid_entrypoint_is_rejected_on_its_own() {
        // Production mutation caught: accepting an entrypoint that escapes with parent components.
        let json = json!({
            "version": 1,
            "platform": "aarch64-apple-darwin",
            "components": [{
                "id": "runtime",
                "version": "1",
                "platform": "aarch64-apple-darwin",
                "url": "https://example.invalid/runtime.tar",
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "downloadBytes": 1,
                "installedBytes": 1,
                "entrypoint": "foo/../hongguo-ai"
            }]
        });
        assert_eq!(
            parse_manifest(&json.to_string()).unwrap_err().code,
            "AI_COMPONENT_MANIFEST_INVALID"
        );
    }

    #[test]
    fn unsupported_component_platform_maps_to_unsupported_platform() {
        // Production mutation caught: collapsing a bad component.platform into MANIFEST_INVALID.
        let json = json!({
            "version": 1,
            "platform": "aarch64-apple-darwin",
            "components": [{
                "id": "runtime",
                "version": "1",
                "platform": "x86_64-apple-darwin",
                "url": "https://example.invalid/runtime.tar",
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "downloadBytes": 1,
                "installedBytes": 1,
                "entrypoint": "hongguo-ai"
            }]
        });
        assert_eq!(
            parse_manifest(&json.to_string()).unwrap_err().code,
            "AI_COMPONENT_UNSUPPORTED_PLATFORM"
        );
    }

    #[test]
    fn download_client_does_not_follow_http_redirects() {
        // Production mutation caught: following https redirects onto an http URL.
        let client = component_download_client(None).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let thread = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let body = b"redirected";
                let header = format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/next\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        let result = client
            .get(format!("http://127.0.0.1:{port}/runtime.tar"))
            .send()
            .unwrap();
        assert!(result.status().is_redirection());
        thread.join().unwrap();
    }

    #[test]
    fn component_redirects_only_follow_https_to_https() {
        let previous = Url::parse("https://github.com/tztmr/hongguo-download").unwrap();
        let https = Url::parse("https://release-assets.githubusercontent.com/file").unwrap();
        let http = Url::parse("http://127.0.0.1/file").unwrap();
        assert!(is_safe_component_redirect(Some(&previous), &https));
        assert!(!is_safe_component_redirect(Some(&previous), &http));
        assert!(!is_safe_component_redirect(None, &https));
    }

    #[test]
    fn network_configuration_resolves_component_mirror_and_accepts_socks_proxy() {
        let fixture = Fixture::new();
        let manager =
            fixture.manager("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        manager
            .configure_network(
                Some("socks5://127.0.0.1:7890"),
                Some("https://mirror.example/ai-components"),
            )
            .unwrap();
        assert_eq!(
            manager.resolve_download_url("https://github.com/org/release/runtime.tar.gz"),
            "https://mirror.example/ai-components/runtime.tar.gz"
        );
        let proxy = manager.download_proxy.lock().unwrap().clone();
        assert_eq!(proxy.as_ref().and_then(Url::host_str), Some("127.0.0.1"));
    }

    #[test]
    fn network_configuration_rejects_insecure_mirror() {
        let fixture = Fixture::new();
        let manager =
            fixture.manager("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let error = manager
            .configure_network(None, Some("http://mirror.example/ai"))
            .unwrap_err();
        assert_eq!(error.code, "AI_COMPONENT_NETWORK_INVALID");
    }

    #[test]
    fn poisoned_in_use_lock_fails_closed() {
        // Production mutation caught: treating a poisoned in-use lock as not in use.
        let fixture = Fixture::new();
        let (good, sha) = archive_with_entrypoint(&fixture.root, "exit 0");
        let manager = fixture.manager(&sha);
        manager
            .install_bytes("runtime", &good, &sha, |_| {})
            .unwrap();
        manager.poison_in_use();
        assert_eq!(
            manager.remove("runtime").unwrap_err().code,
            "AI_COMPONENT_IN_USE"
        );
        assert_eq!(manager.installed_version("runtime").as_deref(), Some("1"));
    }
}
