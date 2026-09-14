use super::source::{Candidate, Episode};
use crate::{youtube::duplicates::inferred_season, AppError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
pub fn flag(v: &Value, key: &str) -> bool {
    v[key].as_bool().unwrap_or(false)
}
pub fn number(v: &Value, key: &str, fallback: u64) -> u64 {
    v[key]
        .as_u64()
        .or_else(|| text(v, key).parse().ok())
        .unwrap_or(fallback)
}
pub fn fingerprint(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))[..24].to_owned()
}

/// Ordered fallback models; a legacy single-model setting remains single-model.
pub fn cover_models(config: &Value) -> Vec<String> {
    let mut models = Vec::new();
    if text(config, "coverSource") == "moyuu" {
        if let Some(values) = config["coverModels"].as_array() {
            for raw in values.iter().filter_map(Value::as_str) {
                let model = raw.trim();
                if !model.is_empty() && !models.iter().any(|m| m == model) {
                    models.push(model.to_owned());
                }
                if models.len() == 4 {
                    break;
                }
            }
        }
    }
    if models.is_empty() {
        let model = text(
            config,
            if text(config, "coverSource") == "jucodex" {
                "jucodexImageModel"
            } else {
                "coverModel"
            },
        )
        .trim();
        if !model.is_empty() {
            models.push(model.to_owned());
        }
    }
    models
}

pub fn validate_config(value: Value) -> Result<Value, AppError> {
    let allowed = [
        "uploadFormat",
        "firstEpisodeShorts",
        "interval",
        "types",
        "scope",
        "orientation",
        "keywords",
        "exclude",
        "completeOnly",
        "definition",
        "concurrency",
        "separate",
        "subtitles",
        "subtitleSource",
        "subtitleFormat",
        "retries",
        "channel",
        "privacy",
        "title",
        "description",
        "tags",
        "aiMetadataApplied",
        "nonAiTitle",
        "nonAiDescription",
        "nonAiTags",
        "coverSource",
        "metadataSource",
        "metadataVersion",
        "textModel",
        "coverModel",
        "category",
        "textPrompt",
        "coverPrompt",
        "duplicate",
        "deleteEpisodes",
        "deleteFinal",
        "keepSubtitles",
        "minDisk",
        "resume",
        "notify",
        "imageSize",
        "imageMode",
        "jucodexBaseUrl",
        "jucodexTextModel",
        "jucodexImageModel",
        "outputLanguage",
        "audience",
        "titleStyle",
    ];
    let mut clean = serde_json::Map::new();
    for key in allowed {
        if let Some(v) = value.get(key) {
            if (v.is_string() && v.as_str().unwrap().len() <= 20000)
                || v.is_boolean()
                || v.is_number()
                || key == "types"
            {
                clean.insert(key.into(), v.clone());
            }
        }
    }
    if let Some(raw) = value.get("coverModels") {
        let values = raw.as_array().ok_or_else(|| {
            AppError::new("AUTOMATION_SETTINGS_INVALID", "封面模型必须为有序列表")
        })?;
        if values.len() > 4
            || values.iter().any(|v| {
                v.as_str().is_none_or(|m| {
                    let m = m.trim();
                    m.is_empty()
                        || m.len() > 200
                        || !m
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._:/-".contains(&b))
                })
            })
        {
            return Err(AppError::new(
                "AUTOMATION_SETTINGS_INVALID",
                "请选择最多 4 个有效封面模型",
            ));
        }
        let mut models = Vec::new();
        for model in values.iter().map(|v| v.as_str().unwrap().trim()) {
            if !models.contains(&model) {
                models.push(model);
            }
        }
        clean.insert("coverModels".into(), serde_json::json!(models));
    }
    let mut c = Value::Object(clean);
    for (key, options) in [
        ("uploadFormat", vec!["auto", "shorts", "standard"]),
        ("scope", vec!["today", "new", "all"]),
        ("orientation", vec!["all", "vertical", "horizontal"]),
        ("privacy", vec!["private", "public", "unlisted"]),
        ("definition", vec!["auto", "1080p", "720p"]),
        ("subtitleSource", vec!["original", "vocal"]),
        ("subtitleFormat", vec!["srt", "vtt"]),
        ("metadataSource", vec!["template", "deepseek", "jucodex"]),
        ("coverSource", vec!["source", "moyuu", "jucodex"]),
        ("imageMode", vec!["reference", "text"]),
    ] {
        if !options.contains(&text(&c, key)) {
            return Err(AppError::new(
                "AUTOMATION_SETTINGS_INVALID",
                format!("自动追剧设置无效：{key}"),
            ));
        }
    }
    if text(&c, "channel").is_empty()
        || text(&c, "title").trim().is_empty()
        || c["types"].as_array().is_none_or(|a| {
            a.is_empty()
                || a.iter()
                    .any(|v| !["漫剧", "AI剧"].contains(&v.as_str().unwrap_or("")))
        })
    {
        return Err(AppError::new(
            "AUTOMATION_SETTINGS_INVALID",
            "请选择频道、监听类型并填写标题模板",
        ));
    }
    for (key, min, max) in [
        ("interval", 1, 60),
        ("concurrency", 1, 3),
        ("retries", 0, 5),
        ("minDisk", 1, 1000),
    ] {
        let n = number(&c, key, u64::MAX);
        if n < min || n > max {
            return Err(AppError::new(
                "AUTOMATION_SETTINGS_INVALID",
                format!("自动追剧设置超出范围：{key}"),
            ));
        }
    }
    if !["1", "22", "24"].contains(&text(&c, "category")) {
        return Err(AppError::new("AUTOMATION_SETTINGS_INVALID", "视频分类无效"));
    }
    if flag(&c, "subtitles") && text(&c, "subtitleSource") == "vocal" && !flag(&c, "separate") {
        return Err(AppError::new(
            "AUTOMATION_SETTINGS_INVALID",
            "使用人声识别字幕需要启用音轨分离",
        ));
    }
    // Never apply an example's generated metadata to every discovered series.
    if flag(&c, "aiMetadataApplied") {
        for (dest, src) in [
            ("title", "nonAiTitle"),
            ("description", "nonAiDescription"),
            ("tags", "nonAiTags"),
        ] {
            let s = text(&c, src).to_owned();
            c[dest] = json!(s);
        }
    }
    c["aiMetadataApplied"] = json!(false);
    c["duplicate"] = json!(true);
    Ok(c)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    #[default]
    Stopped,
    Running,
    Paused,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Pending,
    Working,
    Review,
    Failed,
    Completed,
    Skipped,
    Observing,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedFile {
    pub path: PathBuf,
    pub size: u64,
    pub hash: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DurationCheck {
    pub path: PathBuf,
    pub size: u64,
    pub modified_unix_nanos: u128,
    pub seconds: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    #[serde(default)]
    pub duration_check: Option<DurationCheck>,
    /// Persist admission before the first download, including failed attempts.
    #[serde(default)]
    pub download_admitted: bool,
    #[serde(default)]
    pub cleanup_version: u32,
    pub id: String,
    pub title: String,
    pub book_id: String,
    pub season: Option<u32>,
    pub stage: String,
    pub status: Status,
    pub message: String,
    #[serde(default)]
    pub media_state: Option<String>,
    pub progress: f64,
    pub episode_done: usize,
    pub episode_total: usize,
    pub updated_at: u64,
    pub main_video_url: String,
    pub short_video_url: String,
    pub config: Value,
    pub source: Candidate,
    pub root: PathBuf,
    pub episodes: Vec<Episode>,
    pub files: Vec<PathBuf>,
    pub owned: Vec<OwnedFile>,
    pub merge_job: Option<String>,
    pub separate_job: Option<String>,
    pub subtitle_job: Option<String>,
    pub short_merge_job: Option<String>,
    pub merged: Option<PathBuf>,
    pub prepared: Option<PathBuf>,
    pub subtitle: Option<PathBuf>,
    pub cover: Option<PathBuf>,
    pub short_path: Option<PathBuf>,
    pub metadata: Option<Value>,
    pub short_metadata: Option<Value>,
    pub main_done: bool,
    pub short_done: bool,
    pub allow_duplicate: bool,
    pub attempts: u64,
    pub retry_at: u64,
    #[serde(default)]
    pub ai_started: bool,
    #[serde(default)]
    pub retry_ready: bool,
}
impl Task {
    pub fn new(source: Candidate, config: Value, save_dir: PathBuf) -> Self {
        let season = inferred_season(None, &source.title, "");
        let id = fingerprint(&format!(
            "{}|{}|{:?}",
            text(&config, "channel"),
            source.book_id,
            season
        ));
        let name = crate::sanitize_name(&source.title)
            .chars()
            .take(55)
            .collect::<String>();
        let root = save_dir.join("自动追剧").join(format!(
            "{}__{}__S{}__{}",
            name,
            crate::sanitize_name(&source.book_id),
            season
                .map(|n| n.to_string())
                .unwrap_or_else(|| "未知".into()),
            &id[..8]
        ));
        Self {
            duration_check: None,
            download_admitted: false,
            cleanup_version: 0,
            id,
            title: source.title.clone(),
            book_id: source.book_id.clone(),
            season,
            stage: "inspect".into(),
            status: Status::Pending,
            message: "等待检查源目录与频道记录".into(),
            media_state: None,
            progress: 0.0,
            episode_done: 0,
            episode_total: 0,
            updated_at: now(),
            main_video_url: String::new(),
            short_video_url: String::new(),
            config,
            source,
            root,
            episodes: vec![],
            files: vec![],
            owned: vec![],
            merge_job: None,
            separate_job: None,
            subtitle_job: None,
            short_merge_job: None,
            merged: None,
            prepared: None,
            subtitle: None,
            cover: None,
            short_path: None,
            metadata: None,
            short_metadata: None,
            main_done: false,
            short_done: false,
            allow_duplicate: false,
            attempts: 0,
            retry_at: 0,
            ai_started: false,
            retry_ready: false,
        }
    }
    pub fn terminal(&self) -> bool {
        matches!(
            self.status,
            Status::Completed | Status::Skipped | Status::Review | Status::Failed
        )
    }
    pub fn defer_retry(&mut self) {
        // The configured count controls quick retries, not whether unattended
        // work is abandoned. After that, leave the job eligible after cooldown.
        if self.attempts <= number(&self.config, "retries", 3) {
            self.status = Status::Pending;
            self.retry_at =
                now() + (30 * (1u64 << self.attempts.saturating_sub(1).min(5))).min(900);
        } else {
            self.status = Status::Observing;
            self.retry_at = now() + 900;
        }
        self.retry_ready = true;
    }
    pub fn next(&mut self, stage: &str, message: &str) {
        self.media_state = None;
        self.stage = stage.into();
        self.message = message.into();
        self.status = Status::Pending;
        self.attempts = 0;
        self.retry_at = 0;
        self.retry_ready = false;
        self.progress = 0.0;
    }
    pub fn upload_id(&self, short: bool) -> String {
        format!("auto-{}-{}", self.id, if short { "short" } else { "main" })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Log {
    pub at: u64,
    pub job_id: Option<String>,
    pub message: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub config: Option<Value>,
    pub mode: Mode,
    pub jobs: Vec<Task>,
    #[serde(default)]
    pub waiting: Vec<Task>,
    pub logs: Vec<Log>,
    pub last_scan: u64,
    pub next_scan: u64,
    pub warning: String,
    #[serde(default)]
    pub key_status: KeyStatus,
    #[serde(default)]
    pub cursor_type: usize,
    #[serde(default)]
    pub cursor: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeyStatus {
    pub text: bool,
    pub image: bool,
}
impl Snapshot {
    pub fn log(&mut self, id: Option<String>, message: impl Into<String>) {
        self.logs.push(Log {
            at: now(),
            job_id: id,
            message: message.into(),
        });
        if self.logs.len() > 300 {
            self.logs.drain(..self.logs.len() - 300);
        }
    }
}
