use super::source::{Candidate, Episode};
use crate::{youtube::duplicates::inferred_season, AppError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static LAST_QUEUE_ORDER: AtomicU64 = AtomicU64::new(0);

fn next_queue_order() -> u64 {
    let wall_clock = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    loop {
        let previous = LAST_QUEUE_ORDER.load(Ordering::Relaxed);
        let next = wall_clock.max(previous.saturating_add(1));
        if LAST_QUEUE_ORDER
            .compare_exchange(previous, next, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return next;
        }
    }
}

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
        "categoryIds",
        "exclude",
        "completeOnly",
        "maxEpisodes",
        "collectRecommend",
        "collectNew",
        "collectRank",
        "collectSearch",
        "collectPages",
        "recommendDevices",
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
    if let Some(raw) = value.get("categoryIds") {
        let values = raw
            .as_array()
            .ok_or_else(|| AppError::new("AUTOMATION_SETTINGS_INVALID", "分类标签必须为列表"))?;
        if values.len() > 200
            || values.iter().any(|v| {
                v.as_str().is_none_or(|label| {
                    let label = label.trim();
                    label.is_empty() || label.len() > 200
                })
            })
        {
            return Err(AppError::new(
                "AUTOMATION_SETTINGS_INVALID",
                "分类标签数量或内容无效",
            ));
        }
        let mut ids = Vec::new();
        for id in values.iter().map(|v| v.as_str().unwrap().trim()) {
            if !ids.iter().any(|saved| saved == &id) {
                ids.push(id);
            }
        }
        clean.insert("categoryIds".into(), serde_json::json!(ids));
    }
    let mut c = Value::Object(clean);
    for key in [
        "collectRecommend",
        "collectNew",
        "collectRank",
        "collectSearch",
    ] {
        if c.get(key).is_none() {
            c[key] = json!(true);
        }
        if !c[key].is_boolean() {
            return Err(AppError::new(
                "AUTOMATION_SETTINGS_INVALID",
                "采集来源设置无效",
            ));
        }
    }
    if ![
        "collectRecommend",
        "collectNew",
        "collectRank",
        "collectSearch",
    ]
    .iter()
    .any(|k| flag(&c, k))
    {
        return Err(AppError::new(
            "AUTOMATION_SETTINGS_INVALID",
            "请至少选择一种采集来源",
        ));
    }
    if !["collectRecommend", "collectNew", "collectRank"]
        .iter()
        .any(|k| flag(&c, k))
        && text(&c, "keywords")
            .trim_matches([',', '，', ' '])
            .is_empty()
    {
        return Err(AppError::new(
            "AUTOMATION_SETTINGS_INVALID",
            "仅使用关键词搜索时，请填写包含关键词",
        ));
    }
    for (key, value) in [("collectPages", "10"), ("recommendDevices", "3")] {
        if c.get(key).is_none() {
            c[key] = json!(value);
        }
    }
    if value.get("maxEpisodes").is_none() {
        c["maxEpisodes"] = json!("300");
    }
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
        ("maxEpisodes", 0, 10000),
        ("collectPages", 1, 30),
        ("recommendDevices", 1, 10),
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
    pub manual_skip: bool,
    #[serde(default)]
    pub duration_check: Option<DurationCheck>,
    /// Persist admission before the first download, including failed attempts.
    #[serde(default)]
    pub download_admitted: bool,
    #[serde(default)]
    pub cleanup_version: u32,
    /// Stable admission order. Unlike `updated_at`, this never changes when
    /// progress, retries, or a media stage updates the task.
    #[serde(default)]
    pub queue_order: u64,
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
            manual_skip: false,
            duration_check: None,
            download_admitted: false,
            cleanup_version: 0,
            queue_order: next_queue_order(),
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
    pub fn skip_manually(&mut self) {
        self.manual_skip = true;
        self.status = Status::Skipped;
        self.retry_at = 0;
        self.retry_ready = false;
        self.attempts = 0;
        self.media_state = None;
        self.message =
            "已手动跳过，不再自动处理或上传；跳过后删除本地任务文件夹，保留记录与去重标记".into();
    }
    // In-flight stages may finish after a skip. Keep their file receipts and
    // side-effect handles, but never let their stale status undo the skip.
    pub fn accept_progress(&mut self, incoming: &Task) {
        let skipped = self.manual_skip || incoming.manual_skip;
        *self = incoming.clone();
        if skipped {
            self.skip_manually();
        }
    }
    pub fn defer_retry(&mut self) {
        if self.stage == "download" {
            self.status = Status::Pending;
            let stagger = self.id.bytes().next().unwrap_or(0) as u64 % 16;
            self.retry_at =
                now() + (30 * (1u64 << self.attempts.saturating_sub(1).min(4))).min(300) + stagger;
            self.retry_ready = true;
            return;
        }
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

    /// A Shorts flow remains part of the cleanup contract once it was enabled
    /// or any of its durable outputs/handles were recorded. This protects
    /// in-flight tasks when an older snapshot omitted the setting or a later
    /// settings edit no longer reflects the task's original intent.
    pub fn shorts_required(&self) -> bool {
        flag(&self.config, "firstEpisodeShorts")
            || self.short_merge_job.is_some()
            || self.short_path.is_some()
            || self.short_metadata.is_some()
            || !self.short_video_url.is_empty()
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
    #[serde(default)]
    pub scan_summary: ScanSummary,
    #[serde(default)]
    pub discovery: super::discovery::Discovery,
    #[serde(default)]
    pub discovery_pending: Vec<Candidate>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub checked: usize,
    pub filtered: usize,
    pub known: usize,
    pub added: usize,
    pub more: bool,
    pub at: u64,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub page: u64,
    #[serde(default)]
    pub device_round: u64,
    #[serde(default)]
    pub buffered: usize,
    #[serde(default)]
    pub note: String,
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
