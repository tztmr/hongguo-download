//! Manage channel videos and restrictions; verify ownership before mutations.
use super::config::SecretString;
use crate::AppError;
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashSet, time::Duration};

const BASE: &str = "https://www.googleapis.com/youtube/v3";
const VIDEO_PARTS: &str = "snippet,status,contentDetails,fileDetails";
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedVideo {
    pub id: String,
    pub etag: String,
    pub title: String,
    pub description: String,
    pub privacy_status: String,
    pub thumbnail_url: String,
    pub published_at: String,
    pub video_format: String,
    pub duration_seconds: Option<f64>,
    pub restriction: VideoRestriction,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoRestriction {
    pub kind: String,
    pub reason: String,
    // None and an empty allowed list have different meanings on YouTube.
    pub allowed_regions: Option<Vec<String>>,
    pub blocked_regions: Vec<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoLookup {
    pub items: Vec<ManagedVideo>,
    pub failures: Vec<VideoLookupFailure>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoLookupFailure {
    pub video_id: String,
    pub message: String,
}
pub(super) fn apply_upload_formats(
    channel_id: &str,
    videos: &mut [ManagedVideo],
    known: &[super::duplicates::KnownVideo],
) {
    for video in videos {
        if let Some(record) = known.iter().rev().find(|record| {
            record.channel_id == channel_id
                && record.video_id == video.id
                && !record.upload_format.is_auto()
        }) {
            video.video_format = match record.upload_format {
                super::format::UploadFormat::Shorts => "shorts",
                super::format::UploadFormat::Standard => "standard",
                super::format::UploadFormat::Auto => continue,
            }
            .into();
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoPage {
    pub items: Vec<ManagedVideo>,
    pub next_page_token: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoUpdate {
    pub channel_id: String,
    pub video_id: String,
    pub etag: String,
    pub title: String,
    pub description: String,
    pub privacy_status: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedPlaylist {
    pub id: String,
    pub title: String,
    pub privacy_status: String,
    pub item_ids: Vec<String>,
}

fn invalid() -> AppError {
    AppError::new(
        "YOUTUBE_MANAGEMENT_INVALID",
        "视频资料无效，请检查标题、简介和可见性",
    )
}
fn response_error() -> AppError {
    AppError::new(
        "YOUTUBE_MANAGEMENT_RESPONSE",
        "YouTube 返回的数据不完整，请刷新重试",
    )
}
fn field(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
fn items(value: &Value) -> Result<&Vec<Value>, AppError> {
    value["items"].as_array().ok_or_else(response_error)
}
fn privacy_valid(value: &str) -> bool {
    matches!(value, "private" | "public" | "unlisted")
}
fn duration_seconds(value: &Value) -> Option<f64> {
    let milliseconds = &value["fileDetails"]["durationMs"];
    if let Some(ms) = milliseconds
        .as_f64()
        .or_else(|| milliseconds.as_str()?.parse().ok())
    {
        if ms.is_finite() && ms > 0.0 {
            return Some(ms / 1000.0);
        }
    }
    // YouTube durations use the day/time subset of ISO 8601 (not calendar months).
    let duration = value["contentDetails"]["duration"].as_str()?;
    let mut total = 0.0;
    let mut number = String::new();
    let mut in_time = false;
    for c in duration.strip_prefix('P')?.chars() {
        if c == 'T' && number.is_empty() && !in_time {
            in_time = true;
        } else if c.is_ascii_digit() || c == '.' {
            number.push(c);
        } else {
            let multiplier = match (in_time, c) {
                (false, 'D') => 86400.0,
                (true, 'H') => 3600.0,
                (true, 'M') => 60.0,
                (true, 'S') => 1.0,
                _ => return None,
            };
            total += number.parse::<f64>().ok()? * multiplier;
            number.clear();
        }
    }
    (number.is_empty() && total.is_finite() && total > 0.0).then_some(total)
}
fn video_format(value: &Value, duration: Option<f64>) -> &'static str {
    if duration.is_some_and(|seconds| seconds > 180.0) {
        return "standard";
    }
    // No Shorts flag is exposed by videos.list. Dimensions indicate eligibility,
    // not YouTube's final classification (historical uploads have different rules).
    let Some(stream) = value["fileDetails"]["videoStreams"]
        .as_array()
        .and_then(|s| s.first())
    else {
        return "unknown";
    };
    let aspect = stream["aspectRatio"].as_f64().or_else(|| {
        let width = stream["widthPixels"].as_f64()?;
        let height = stream["heightPixels"].as_f64()?;
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        match stream["rotation"].as_str().unwrap_or("none") {
            "none" | "upsideDown" => Some(width / height),
            "clockwise" | "counterClockwise" => Some(height / width),
            _ => None,
        }
    });
    match aspect.filter(|a| a.is_finite() && *a > 0.0) {
        Some(ratio) if ratio > 1.0 => "standard",
        Some(_)
            if duration.is_some() && field(value, "/snippet/liveBroadcastContent") == "none" =>
        {
            "shortsCandidate"
        }
        _ => "unknown",
    }
}
fn restriction(value: &Value) -> VideoRestriction {
    let regions = |pointer: &str| {
        value
            .pointer(pointer)
            .and_then(Value::as_array)
            .and_then(|rows| {
                rows.iter()
                    .map(|value| value.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
            })
    };
    let allowed = regions("/contentDetails/regionRestriction/allowed");
    let blocked = regions("/contentDetails/regionRestriction/blocked").unwrap_or_default();
    let upload = field(value, "/status/uploadStatus");
    let (kind, reason) = if upload == "rejected" {
        match field(value, "/status/rejectionReason").as_str() {
            "copyright" => ("copyright", "版权原因被拒绝"),
            "claim" => ("copyright", "版权声明导致拒绝"),
            _ => ("unavailable", "YouTube 已拒绝此视频，请到 Studio 核对原因"),
        }
    } else if upload == "failed" || upload == "deleted" {
        ("unavailable", "视频处理失败或已不可用")
    } else if allowed.as_ref().is_some_and(Vec::is_empty) {
        ("global", "全球封锁")
    } else if allowed.is_some() || !blocked.is_empty() {
        ("region", "地区限制")
    } else {
        ("noneReported", "API 未返回封锁信息")
    };
    VideoRestriction {
        kind: kind.into(),
        reason: reason.into(),
        allowed_regions: allowed,
        blocked_regions: blocked,
    }
}
fn parse_video(value: &Value) -> Result<ManagedVideo, AppError> {
    let duration = duration_seconds(value);
    let video = ManagedVideo {
        id: field(value, "/id"),
        etag: field(value, "/etag"),
        title: field(value, "/snippet/title"),
        description: field(value, "/snippet/description"),
        privacy_status: field(value, "/status/privacyStatus"),
        thumbnail_url: ["medium", "high", "default"]
            .iter()
            .map(|size| field(value, &format!("/snippet/thumbnails/{size}/url")))
            .find(|s| !s.is_empty())
            .unwrap_or_default(),
        published_at: field(value, "/snippet/publishedAt"),
        video_format: video_format(value, duration).into(),
        duration_seconds: duration,
        restriction: restriction(value),
    };
    if video.id.is_empty() || video.etag.is_empty() || !privacy_valid(&video.privacy_status) {
        return Err(response_error());
    }
    Ok(video)
}
fn owned(value: &Value, channel_id: &str) -> Result<(), AppError> {
    if field(value, "/snippet/channelId") != channel_id {
        return Err(AppError::new(
            "YOUTUBE_CHANNEL_MISMATCH",
            "视频或播放列表不属于所选频道",
        ));
    }
    Ok(())
}
fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}
fn copy_fields(source: &Value, keys: &[&str]) -> Value {
    let mut out = json!({});
    for key in keys {
        if let Some(value) = source.get(*key) {
            out[*key] = value.clone();
        }
    }
    out
}
fn update_body(current: &Value, request: &VideoUpdate) -> Result<(Value, String), AppError> {
    if request.title.trim().is_empty()
        || request.title.chars().count() > 100
        || request.description.len() > 5000
        || request.title.contains(['<', '>'])
        || request.description.contains(['<', '>'])
        || !privacy_valid(&request.privacy_status)
    {
        return Err(invalid());
    }
    owned(current, &request.channel_id)?;
    if field(current, "/id") != request.video_id
        || request.etag.is_empty()
        || field(current, "/etag") != request.etag
    {
        return Err(AppError::new(
            "YOUTUBE_VIDEO_CHANGED",
            "视频资料已发生变化，请重新打开编辑后再保存",
        ));
    }
    let mut body = json!({"id": request.video_id});
    let mut parts = Vec::new();
    if field(current, "/snippet/title") != request.title.trim()
        || field(current, "/snippet/description") != request.description
    {
        let mut snippet = copy_fields(
            &current["snippet"],
            &[
                "title",
                "description",
                "categoryId",
                "tags",
                "defaultLanguage",
                "defaultAudioLanguage",
            ],
        );
        if field(&snippet, "/categoryId").is_empty() {
            return Err(response_error());
        }
        snippet["title"] = json!(request.title.trim());
        snippet["description"] = json!(request.description);
        body["snippet"] = snippet;
        parts.push("snippet");
    }
    if field(current, "/status/privacyStatus") != request.privacy_status {
        let mut status = copy_fields(
            &current["status"],
            &[
                "privacyStatus",
                "license",
                "embeddable",
                "publicStatsViewable",
                "selfDeclaredMadeForKids",
                "containsSyntheticMedia",
                "publishAt",
            ],
        );
        if request.privacy_status != "private" && status.get("publishAt").is_some() {
            return Err(AppError::new(
                "YOUTUBE_SCHEDULED_VIDEO",
                "视频已设置定时发布，请先在 YouTube Studio 调整发布时间后再修改可见性",
            ));
        }
        status["privacyStatus"] = json!(request.privacy_status);
        body["status"] = status;
        parts.push("status");
    }
    Ok((body, parts.join(",")))
}

pub struct ManagementApi {
    client: Client,
    base: String,
    token: SecretString,
    channel_id: String,
}
impl ManagementApi {
    pub fn new(channel_id: &str, token: SecretString) -> Result<Self, AppError> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(45))
            .build()
            .map_err(|_| response_error())?;
        Ok(Self {
            client,
            base: BASE.into(),
            token,
            channel_id: channel_id.into(),
        })
    }
    async fn request(
        &self,
        method: Method,
        resource: &str,
        params: &[(&str, &str)],
        body: Option<&Value>,
        etag: Option<&str>,
    ) -> Result<Value, AppError> {
        let mut request = self
            .client
            .request(method, format!("{}/{resource}", self.base))
            .query(params)
            .bearer_auth(self.token.expose_secret());
        if let Some(body) = body {
            request = request.json(body);
        }
        if let Some(etag) = etag {
            request = request.header("If-Match", etag);
        }
        let response = request.send().await.map_err(|_| {
            AppError::new(
                "YOUTUBE_MANAGEMENT_NETWORK",
                "连接 YouTube 失败，请检查网络后刷新确认最新状态",
            )
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(json!({}));
        }
        let data: Value = response.json().await.map_err(|_| response_error())?;
        if status.is_success() {
            return Ok(data);
        }
        let reason = field(&data, "/error/errors/0/reason");
        let (code, message) = match (status.as_u16(), reason.as_str()) {
            (401, _) | (_, "insufficientPermissions") => (
                "AUTH_REQUIRED",
                "需要视频管理权限，请在设置中重新授权 YouTube 频道",
            ),
            (412, _) => (
                "YOUTUBE_VIDEO_CHANGED",
                "视频资料已发生变化，请重新打开编辑后再保存",
            ),
            (_, "quotaExceeded" | "dailyLimitExceeded") => (
                "YOUTUBE_QUOTA_EXCEEDED",
                "YouTube 接口配额已用完，请稍后重试",
            ),
            (403, _) => (
                "YOUTUBE_MANAGEMENT_FORBIDDEN",
                "YouTube 拒绝了操作，请检查频道权限；旧授权可在设置中重新授权",
            ),
            (404, _) => (
                "YOUTUBE_VIDEO_NOT_FOUND",
                "视频或播放列表已不存在，请刷新列表",
            ),
            _ => (
                "YOUTUBE_MANAGEMENT_FAILED",
                "YouTube 操作失败，请检查资料并刷新确认最新状态后重试",
            ),
        };
        Err(AppError::new(code, message))
    }
    async fn get(&self, resource: &str, params: &[(&str, &str)]) -> Result<Value, AppError> {
        self.request(Method::GET, resource, params, None, None)
            .await
    }
    async fn channel_uploads(&self) -> Result<String, AppError> {
        let data = self
            .get("channels", &[("part", "contentDetails"), ("mine", "true")])
            .await?;
        let channel = items(&data)?
            .iter()
            .find(|v| field(v, "/id") == self.channel_id)
            .ok_or_else(|| {
                AppError::new(
                    "YOUTUBE_CHANNEL_MISMATCH",
                    "授权频道与所选频道不一致，请重新授权",
                )
            })?;
        let id = field(channel, "/contentDetails/relatedPlaylists/uploads");
        if id.is_empty() {
            return Err(response_error());
        }
        Ok(id)
    }
    pub async fn list(&self, page_token: &str) -> Result<VideoPage, AppError> {
        let uploads = self.channel_uploads().await?;
        let page = self
            .get(
                "playlistItems",
                &[
                    ("part", "contentDetails"),
                    ("playlistId", &uploads),
                    ("maxResults", "50"),
                    ("pageToken", page_token),
                ],
            )
            .await?;
        let ids = items(&page)?
            .iter()
            .map(|v| field(v, "/contentDetails/videoId"))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(",");
        let mut videos = Vec::new();
        if !ids.is_empty() {
            let data = self
                .get("videos", &[("part", VIDEO_PARTS), ("id", &ids)])
                .await?;
            for video in items(&data)? {
                owned(video, &self.channel_id)?;
                videos.push(parse_video(video)?);
            }
        }
        let next = field(&page, "/nextPageToken");
        if !next.is_empty() && next == page_token {
            return Err(response_error());
        }
        Ok(VideoPage {
            items: videos,
            next_page_token: (!next.is_empty()).then_some(next),
        })
    }
    pub async fn video(&self, video_id: &str) -> Result<Value, AppError> {
        let data = self
            .get("videos", &[("part", VIDEO_PARTS), ("id", video_id)])
            .await?;
        let video = items(&data)?
            .iter()
            .find(|video| field(video, "/id") == video_id)
            .ok_or_else(|| AppError::new("YOUTUBE_VIDEO_NOT_FOUND", "视频已不存在，请刷新列表"))?
            .clone();
        owned(&video, &self.channel_id)?;
        Ok(video)
    }
    pub async fn detail(&self, video_id: &str) -> Result<ManagedVideo, AppError> {
        parse_video(&self.video(video_id).await?)
    }

    pub async fn lookup(&self, video_ids: &[String]) -> Result<VideoLookup, AppError> {
        if video_ids.is_empty()
            || video_ids.len() > 50
            || video_ids.iter().any(|id| !valid_video_id(id))
        {
            return Err(AppError::new(
                "YOUTUBE_LOOKUP_INVALID",
                "每次可查询 1～50 个有效的视频 ID",
            ));
        }
        self.channel_uploads().await?;
        let mut seen = HashSet::new();
        let ids: Vec<_> = video_ids
            .iter()
            .filter(|id| seen.insert(id.as_str()))
            .collect();
        // Read public parts first so one link from another channel cannot prevent
        // querying the rest. Never return a foreign channel's video for deletion.
        let data = self
            .get(
                "videos",
                &[
                    ("part", "snippet,status,contentDetails"),
                    (
                        "id",
                        &ids.iter()
                            .map(|id| id.as_str())
                            .collect::<Vec<_>>()
                            .join(","),
                    ),
                ],
            )
            .await?;
        let mut result = VideoLookup {
            items: vec![],
            failures: vec![],
        };
        for id in ids {
            let parsed = items(&data)?
                .iter()
                .find(|v| field(v, "/id") == *id)
                .ok_or_else(|| {
                    AppError::new(
                        "YOUTUBE_VIDEO_NOT_FOUND",
                        "视频不存在或当前授权无法读取，请到 Studio 核对",
                    )
                })
                .and_then(|v| {
                    owned(v, &self.channel_id)?;
                    parse_video(v)
                });
            match parsed {
                Ok(video) => result.items.push(video),
                Err(error) => result.failures.push(VideoLookupFailure {
                    video_id: id.clone(),
                    message: error.message,
                }),
            }
        }
        // Use the same parts as the editor so metadata and its revision stay
        // together. Owner-only file metadata may be unavailable for blocked or
        // newly uploaded videos; that must not hide the initial restriction read.
        if !result.items.is_empty() {
            let owned_ids = result
                .items
                .iter()
                .map(|video| video.id.as_str())
                .collect::<Vec<_>>()
                .join(",");
            if let Ok(details) = self
                .get("videos", &[("part", VIDEO_PARTS), ("id", &owned_ids)])
                .await
            {
                if let Some(rows) = details["items"].as_array() {
                    for video in &mut result.items {
                        if let Some(detail) = rows.iter().find(|v| field(v, "/id") == video.id) {
                            owned(detail, &self.channel_id)?;
                            if let Ok(fresh) = parse_video(detail) {
                                *video = fresh;
                            }
                        }
                    }
                }
            }
        }
        Ok(result)
    }

    pub async fn delete_video(&self, video_id: &str) -> Result<(), AppError> {
        if !valid_video_id(video_id) {
            return Err(AppError::new(
                "YOUTUBE_VIDEO_ID_INVALID",
                "无效的视频 ID，请刷新后重新选择",
            ));
        }
        // Verify the token's channel and this exact video's owner before DELETE.
        self.channel_uploads().await?;
        self.video(video_id).await?;
        self.request(Method::DELETE, "videos", &[("id", video_id)], None, None)
            .await?;
        Ok(())
    }

    /// Upload transport completion does not establish that YouTube finished
    /// processing the video. Cleanup must wait for this separate owner read.
    pub async fn processing_ready(&self, video_id: &str) -> Result<bool, AppError> {
        if video_id.trim().is_empty() {
            return Err(invalid());
        }
        let data = self
            .get(
                "videos",
                &[
                    ("part", "snippet,status,processingDetails"),
                    ("id", video_id),
                ],
            )
            .await?;
        let videos = items(&data)?;
        if videos.is_empty() {
            return Err(AppError::new(
                "YOUTUBE_VIDEO_NOT_FOUND",
                "视频已不存在，已暂停自动清理并保留本地文件",
            ));
        }
        let video = videos
            .iter()
            .find(|video| field(video, "/id") == video_id)
            .ok_or_else(response_error)?;
        owned(video, &self.channel_id)?;
        let processing = field(video, "/processingDetails/processingStatus");
        let upload = field(video, "/status/uploadStatus");
        if [&processing, &upload].iter().any(|state| {
            matches!(
                state.as_str(),
                "failed" | "terminated" | "rejected" | "deleted"
            )
        }) {
            return Err(AppError::new(
                "YOUTUBE_PROCESSING_FAILED",
                "YouTube 视频处理失败、已终止或视频不可用；已保留本地文件，请在 YouTube Studio 核对",
            ));
        }
        Ok(processing == "succeeded"
            || (video.get("processingDetails").is_none() && upload == "processed"))
    }

    pub async fn update(&self, request: &VideoUpdate) -> Result<ManagedVideo, AppError> {
        let current = self.video(&request.video_id).await?;
        let (body, part) = update_body(&current, request)?;
        if !part.is_empty() {
            self.request(
                Method::PUT,
                "videos",
                &[("part", &part)],
                Some(&body),
                Some(&request.etag),
            )
            .await?;
        }
        // Read back actual state, including restrictions enforced by YouTube.
        parse_video(&self.video(&request.video_id).await?)
    }
    async fn all(&self, resource: &str, params: &[(&str, &str)]) -> Result<Vec<Value>, AppError> {
        let mut results = Vec::new();
        let mut page_token = String::new();
        let mut seen = HashSet::new();
        loop {
            let mut query = params.to_vec();
            query.extend([("maxResults", "50"), ("pageToken", &page_token)]);
            let page = self.get(resource, &query).await?;
            results.extend(items(&page)?.iter().cloned());
            let next = field(&page, "/nextPageToken");
            if next.is_empty() {
                return Ok(results);
            }
            if !seen.insert(next.clone()) {
                return Err(response_error());
            }
            page_token = next;
        }
    }
    async fn membership_ids(
        &self,
        playlist_id: &str,
        video_id: &str,
    ) -> Result<Vec<String>, AppError> {
        Ok(self
            .all(
                "playlistItems",
                &[
                    ("part", "id"),
                    ("playlistId", playlist_id),
                    ("videoId", video_id),
                ],
            )
            .await?
            .iter()
            .map(|v| field(v, "/id"))
            .collect())
    }
    pub async fn playlists(&self, video_id: &str) -> Result<Vec<ManagedPlaylist>, AppError> {
        self.channel_uploads().await?;
        self.video(video_id).await?;
        let mut result = Vec::new();
        for value in self
            .all("playlists", &[("part", "snippet,status"), ("mine", "true")])
            .await?
        {
            owned(&value, &self.channel_id)?;
            let id = field(&value, "/id");
            let item_ids = self.membership_ids(&id, video_id).await?;
            result.push(ManagedPlaylist {
                id,
                title: field(&value, "/snippet/title"),
                privacy_status: field(&value, "/status/privacyStatus"),
                item_ids,
            });
        }
        Ok(result)
    }
    pub async fn set_membership(
        &self,
        video_id: &str,
        playlist_id: &str,
        included: bool,
    ) -> Result<(), AppError> {
        self.video(video_id).await?;
        let data = self
            .get("playlists", &[("part", "snippet"), ("id", playlist_id)])
            .await?;
        owned(
            items(&data)?.first().ok_or_else(response_error)?,
            &self.channel_id,
        )?;
        let ids = self.membership_ids(playlist_id, video_id).await?;
        if included && ids.is_empty() {
            self.request(Method::POST, "playlistItems", &[("part", "snippet")], Some(&json!({"snippet": {"playlistId": playlist_id, "resourceId": {"kind": "youtube#video", "videoId": video_id}}})), None).await?;
        } else if !included {
            for id in ids {
                self.request(Method::DELETE, "playlistItems", &[("id", &id)], None, None)
                    .await?;
            }
        }
        Ok(())
    }
    pub async fn create_playlist(
        &self,
        title: &str,
        privacy: &str,
    ) -> Result<ManagedPlaylist, AppError> {
        if title.trim().is_empty() || title.chars().count() > 150 || !privacy_valid(privacy) {
            return Err(invalid());
        }
        self.channel_uploads().await?;
        let data = self.request(Method::POST, "playlists", &[("part", "snippet,status")], Some(&json!({"snippet": {"title": title.trim()}, "status": {"privacyStatus": privacy}})), None).await?;
        Ok(ManagedPlaylist {
            id: field(&data, "/id"),
            title: field(&data, "/snippet/title"),
            privacy_status: field(&data, "/status/privacyStatus"),
            item_ids: vec![],
        })
    }
}

impl super::service::YouTubeService {
    pub async fn automation_processing_ready(
        &self,
        channel_id: &str,
        video_id: &str,
    ) -> Result<bool, AppError> {
        self.management_api(channel_id)
            .await?
            .processing_ready(video_id)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn original() -> Value {
        json!({"id":"v1","etag":"rev1","snippet":{"channelId":"c1","title":"旧标题","description":"简介","categoryId":"24","tags":["短剧"],"defaultLanguage":"zh","defaultAudioLanguage":"zh","thumbnails":{}},"status":{"privacyStatus":"private","license":"creativeCommon","embeddable":false,"publicStatsViewable":false,"selfDeclaredMadeForKids":true,"containsSyntheticMedia":true,"uploadStatus":"processed"}})
    }
    fn edit() -> VideoUpdate {
        VideoUpdate {
            channel_id: "c1".into(),
            video_id: "v1".into(),
            etag: "rev1".into(),
            title: "新标题".into(),
            description: "新简介".into(),
            privacy_status: "public".into(),
        }
    }
    #[test]
    fn preserves_unedited_writable_fields_and_omits_readonly_fields() {
        let (body, part) = update_body(&original(), &edit()).unwrap();
        assert_eq!(part, "snippet,status");
        assert_eq!(body["snippet"]["tags"], json!(["短剧"]));
        assert_eq!(body["snippet"]["categoryId"], "24");
        assert_eq!(body["snippet"]["defaultAudioLanguage"], "zh");
        assert!(body["snippet"].get("thumbnails").is_none());
        assert_eq!(body["status"]["selfDeclaredMadeForKids"], true);
        assert_eq!(body["status"]["embeddable"], false);
        assert_eq!(body["status"]["license"], "creativeCommon");
        assert!(body["status"].get("uploadStatus").is_none());
    }
    #[test]
    fn title_only_does_not_write_status_or_cancel_schedule() {
        let mut original = original();
        original["status"]["publishAt"] = json!("2027-01-01T00:00:00Z");
        let mut edit = edit();
        edit.privacy_status = "private".into();
        let (body, part) = update_body(&original, &edit).unwrap();
        assert_eq!(part, "snippet");
        assert!(body.get("status").is_none());
        edit.privacy_status = "public".into();
        assert_eq!(
            update_body(&original, &edit).unwrap_err().code,
            "YOUTUBE_SCHEDULED_VIDEO"
        );
    }
    #[test]
    fn rejects_stale_revision_and_cross_channel_writes() {
        let mut edit = edit();
        edit.etag = "stale".into();
        assert_eq!(
            update_body(&original(), &edit).unwrap_err().code,
            "YOUTUBE_VIDEO_CHANGED"
        );
        edit.channel_id = "other".into();
        assert_eq!(
            update_body(&original(), &edit).unwrap_err().code,
            "YOUTUBE_CHANNEL_MISMATCH"
        );
    }
    #[test]
    fn rejects_invalid_visibility_empty_title_and_description_over_byte_limit() {
        let mut edit = edit();
        edit.privacy_status = "draft".into();
        assert!(update_body(&original(), &edit).is_err());
        edit = self::edit();
        edit.title = " ".into();
        assert!(update_body(&original(), &edit).is_err());
        edit = self::edit();
        edit.description = "中".repeat(1667);
        assert!(update_body(&original(), &edit).is_err());
    }
    async fn server(
        replies: Vec<(u16, Value)>,
    ) -> (ManagementApi, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (status, body) in replies {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buf = [0; 8192];
                    let n = stream.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                        let len = header
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")
                                    .map(|n| n.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8(bytes).unwrap());
                let body = if status == 204 {
                    String::new()
                } else {
                    body.to_string()
                };
                stream.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
            requests
        });
        (
            ManagementApi {
                client: Client::builder().no_proxy().build().unwrap(),
                base,
                token: SecretString::new("test-token"),
                channel_id: "c1".into(),
            },
            task,
        )
    }
    fn channel() -> Value {
        json!({"items":[{"id":"c1","contentDetails":{"relatedPlaylists":{"uploads":"uploads1"}}}]})
    }
    fn identified(id: &str) -> Value {
        let mut video = original();
        video["id"] = json!(id);
        video
    }
    #[test]
    fn restriction_distinguishes_global_regional_and_no_report_without_assuming_copyright() {
        for (region, expected) in [
            (json!({"allowed": []}), "global"),
            (json!({"allowed": ["US"]}), "region"),
            (json!({"blocked": ["US", "CA"]}), "region"),
            (json!({"blocked": []}), "noneReported"),
            (json!({"allowed": [null]}), "noneReported"),
            (Value::Null, "noneReported"),
        ] {
            let mut video = original();
            video["contentDetails"] = json!({"regionRestriction": region, "licensedContent": true});
            let parsed = parse_video(&video).unwrap();
            assert_eq!(parsed.restriction.kind, expected);
            if expected == "global" {
                assert_eq!(parsed.restriction.allowed_regions, Some(vec![]));
            }
            assert_eq!(parsed.privacy_status, "private");
        }
    }
    #[test]
    fn copyright_rejection_requires_rejected_upload_and_does_not_expose_remote_reason() {
        for (upload, reason, kind) in [
            ("rejected", "claim", "copyright"),
            ("rejected", "copyright", "copyright"),
            ("processed", "copyright", "noneReported"),
            ("rejected", "sensitive-error-body", "unavailable"),
            ("failed", "codec", "unavailable"),
        ] {
            let mut video = original();
            video["status"]["uploadStatus"] = json!(upload);
            video["status"]["rejectionReason"] = json!(reason);
            let parsed = parse_video(&video).unwrap();
            assert_eq!(parsed.restriction.kind, kind);
            assert!(!parsed.restriction.reason.contains("sensitive"));
        }
    }
    #[test]
    fn shorts_candidates_need_dimensions_and_duration_not_just_a_short_duration() {
        for (details, duration, kind) in [
            (
                json!({"videoStreams":[{"widthPixels":1080,"heightPixels":1920}]}),
                "PT3M",
                "shortsCandidate",
            ),
            (
                json!({"videoStreams":[{"widthPixels":1080,"heightPixels":1080}]}),
                "PT1M30S",
                "shortsCandidate",
            ),
            (
                json!({"videoStreams":[{"widthPixels":1920,"heightPixels":1080}]}),
                "PT30S",
                "standard",
            ),
            (
                json!({"videoStreams":[{"widthPixels":1920,"heightPixels":1080,"rotation":"clockwise"}]}),
                "PT30S",
                "shortsCandidate",
            ),
            (
                json!({"videoStreams":[{"widthPixels":720,"heightPixels":720,"aspectRatio":2.0}]}),
                "PT30S",
                "standard",
            ),
            (Value::Null, "PT30S", "unknown"),
            (Value::Null, "PT3M0.1S", "standard"),
            (Value::Null, "P1DT2H", "standard"),
            (
                json!({"videoStreams":[{"widthPixels":1080,"heightPixels":1920}]}),
                "PT0S",
                "unknown",
            ),
        ] {
            let mut video = original();
            video["snippet"]["liveBroadcastContent"] = json!("none");
            video["contentDetails"] = json!({"duration": duration});
            video["fileDetails"] = details;
            assert_eq!(parse_video(&video).unwrap().video_format, kind);
        }
        let mut video = original();
        video["contentDetails"] = json!({"duration":"PT3M"});
        video["fileDetails"] = json!({"durationMs":"180001"});
        assert_eq!(duration_seconds(&video), Some(180.001));
    }
    #[test]
    fn upload_format_evidence_is_matched_by_both_channel_and_video_id() {
        use super::super::{duplicates::KnownVideo, format::UploadFormat};
        let mut videos = vec![
            parse_video(&identified("short000001")).unwrap(),
            parse_video(&original()).unwrap(),
        ];
        let known = vec![
            KnownVideo {
                channel_id: "other".into(),
                video_id: "v1".into(),
                upload_format: UploadFormat::Shorts,
                ..Default::default()
            },
            KnownVideo {
                channel_id: "c1".into(),
                video_id: "short000001".into(),
                upload_format: UploadFormat::Shorts,
                ..Default::default()
            },
        ];
        apply_upload_formats("c1", &mut videos, &known);
        assert_eq!(videos[0].video_format, "shorts");
        assert_eq!(videos[1].video_format, "unknown");
    }
    #[tokio::test]
    async fn notification_lookup_deduplicates_and_keeps_valid_results_when_other_links_are_missing_or_foreign(
    ) {
        let mut foreign = identified("other000001");
        foreign["snippet"]["channelId"] = json!("other");
        let mut full = identified("short000001");
        full["etag"] = json!("full-revision");
        full["fileDetails"] = json!({"durationMs":"90000"});
        let (api, server) = server(vec![
            (200, channel()),
            (200, json!({"items":[identified("short000001"), foreign]})),
            (200, json!({"items":[full]})),
        ])
        .await;
        let result = api
            .lookup(
                &["short000001", "other000001", "gone0000001", "short000001"].map(str::to_owned),
            )
            .await
            .unwrap();
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].id, "short000001");
        assert_eq!(result.failures.len(), 2);
        assert_eq!(result.failures[0].video_id, "other000001");
        assert!(result.failures[0].message.contains("不属于"));
        let requests = server.await.unwrap();
        assert!(requests.iter().all(|request| request.starts_with("GET ")));
        assert!(requests[1].contains("id=short000001%2Cother000001%2Cgone0000001 "));
        assert!(requests[2]
            .contains("part=snippet%2Cstatus%2CcontentDetails%2CfileDetails&id=short000001 "));
        assert_eq!(result.items[0].duration_seconds, Some(90.0));
        assert_eq!(result.items[0].etag, "full-revision");
    }
    #[tokio::test]
    async fn delete_verifies_channel_and_exact_video_then_deletes_only_that_video() {
        let (api, server) = server(vec![
            (200, channel()),
            (200, json!({"items":[identified("short000001")]})),
            (204, Value::Null),
        ])
        .await;
        api.delete_video("short000001").await.unwrap();
        let requests = server.await.unwrap();
        assert!(requests[0].starts_with("GET /channels?"));
        assert!(requests[1].contains("id=short000001"));
        assert!(requests[2].starts_with("DELETE /videos?id=short000001 "));
        assert_eq!(requests[2].split("\r\n\r\n").nth(1).unwrap(), "");
    }
    #[tokio::test]
    async fn delete_never_writes_for_a_foreign_missing_or_wrong_video() {
        let mut foreign = identified("short000001");
        foreign["snippet"]["channelId"] = json!("other");
        for (rows, code) in [
            (json!([foreign]), "YOUTUBE_CHANNEL_MISMATCH"),
            (json!([]), "YOUTUBE_VIDEO_NOT_FOUND"),
            (
                json!([identified("other000001")]),
                "YOUTUBE_VIDEO_NOT_FOUND",
            ),
        ] {
            let (api, server) = server(vec![(200, channel()), (200, json!({"items":rows}))]).await;
            assert_eq!(
                api.delete_video("short000001").await.unwrap_err().code,
                code
            );
            assert!(server
                .await
                .unwrap()
                .iter()
                .all(|request| request.starts_with("GET ")));
        }
    }
    #[tokio::test]
    async fn invalid_ids_and_oversize_lookup_are_rejected_before_requests() {
        let (api, server) = server(vec![]).await;
        for id in ["", "short000001,other000001", "bad&fields=id"] {
            assert!(api.delete_video(id).await.is_err());
            assert!(api.lookup(&[id.into()]).await.is_err());
        }
        assert!(api.lookup(&vec!["short000001".into(); 51]).await.is_err());
        assert!(server.await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn delete_failure_returns_actionable_error_instead_of_success() {
        let (api, server) = server(vec![(200, channel()), (200, json!({"items":[identified("short000001")]})),
            (403, json!({"error":{"message":"sensitive-token","errors":[{"reason":"quotaExceeded"}]}}))]).await;
        let error = api.delete_video("short000001").await.unwrap_err();
        assert_eq!(error.code, "YOUTUBE_QUOTA_EXCEEDED");
        assert!(!error.message.contains("sensitive"));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn processing_waits_until_success_and_only_reads_the_requested_video() {
        let states = [
            (json!({"processingStatus":"processing"}), "uploaded", false),
            (json!({"processingStatus":"processing"}), "processed", false),
            (json!({"processingStatus":"unknown"}), "processed", false),
            (json!({}), "processed", false),
            (Value::Null, "processed", false),
            (json!({"processingStatus":"succeeded"}), "uploaded", true),
        ];
        let replies = states
            .iter()
            .map(|(details, upload, _)| {
                (
                    200,
                    json!({"items":[{
                        "id":"v1", "snippet":{"channelId":"c1"},
                        "processingDetails":details,"status":{"uploadStatus":upload}
                    }]}),
                )
            })
            .collect();
        let (api, server) = server(replies).await;
        for (_, _, expected) in states {
            assert_eq!(api.processing_ready("v1").await.unwrap(), expected);
        }
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 6);
        for request in requests {
            assert!(request.starts_with("GET /videos?"));
            assert!(request.contains("part=snippet%2Cstatus%2CprocessingDetails"));
            assert!(request.contains("id=v1"));
            assert_eq!(request.split("\r\n\r\n").nth(1).unwrap_or(""), "");
        }
    }
    #[tokio::test]
    async fn processed_upload_is_ready_only_when_processing_details_are_absent() {
        let (api, server) = server(["uploaded", "processed", "unknown", ""]
            .iter()
            .map(|upload| (200,json!({"items":[{"id":"v1","snippet":{"channelId":"c1"},"status":{"uploadStatus":upload}}]})))
            .collect()).await;
        for expected in [false, true, false, false] {
            assert_eq!(api.processing_ready("v1").await.unwrap(), expected);
        }
        assert!(server
            .await
            .unwrap()
            .iter()
            .all(|request| request.starts_with("GET ")));
    }
    #[tokio::test]
    async fn processing_failure_never_exposes_remote_reason_or_allows_cleanup() {
        let mut replies = Vec::new();
        for state in ["failed", "terminated", "rejected", "deleted"] {
            for processing_failure in [true, false] {
                let mut video = json!({"id":"v1","snippet":{"channelId":"c1"},
                    "processingDetails":{"processingStatus":"succeeded","processingFailureReason":"sensitive-remote-body"},
                    "status":{"uploadStatus":"processed","failureReason":"sensitive-remote-body"}});
                if processing_failure {
                    video["processingDetails"]["processingStatus"] = json!(state);
                } else {
                    video["status"]["uploadStatus"] = json!(state);
                }
                replies.push((200, json!({"items":[video]})));
            }
        }
        let (api, server) = server(replies).await;
        for _ in 0..8 {
            let error = api.processing_ready("v1").await.unwrap_err();
            assert_eq!(error.code, "YOUTUBE_PROCESSING_FAILED");
            assert!(!error.message.contains("sensitive-remote-body"));
        }
        assert!(server
            .await
            .unwrap()
            .iter()
            .all(|request| request.starts_with("GET ")));
    }
    #[tokio::test]
    async fn processing_rejects_cross_channel_missing_and_wrong_video_results() {
        let (api,server)=server(vec![
            (200,json!({"items":[{"id":"v1","snippet":{"channelId":"other"},"processingDetails":{"processingStatus":"succeeded"}}]})),
            (200,json!({"items":[]})),
            (200,json!({"items":[{"id":"v2","snippet":{"channelId":"c1"},"processingDetails":{"processingStatus":"succeeded"}}]})),
        ]).await;
        for expected in [
            "YOUTUBE_CHANNEL_MISMATCH",
            "YOUTUBE_VIDEO_NOT_FOUND",
            "YOUTUBE_MANAGEMENT_RESPONSE",
        ] {
            assert_eq!(api.processing_ready("v1").await.unwrap_err().code, expected);
        }
        assert!(server
            .await
            .unwrap()
            .iter()
            .all(|request| request.starts_with("GET ")));
    }
    #[tokio::test]
    async fn lists_private_videos_using_uploads_playlist_with_page_token() {
        let (api, server) = server(vec![
            (200, channel()),
            (
                200,
                json!({"items":[{"contentDetails":{"videoId":"v1"}}],"nextPageToken":"page3"}),
            ),
            (200, json!({"items":[original()]})),
        ])
        .await;
        let page = api.list("page2").await.unwrap();
        assert_eq!(page.next_page_token.as_deref(), Some("page3"));
        assert_eq!(page.items[0].privacy_status, "private");
        let requests = server.await.unwrap();
        assert!(requests[1].contains("pageToken=page2"));
        assert!(requests[1].contains("playlistId=uploads1"));
        assert!(requests[2].contains("snippet%2Cstatus"));
    }
    #[tokio::test]
    async fn update_sends_preserved_fields_and_revision_then_reads_actual_visibility() {
        let mut actual = original();
        actual["etag"] = json!("rev2");
        actual["snippet"]["title"] = json!("新标题");
        let (api, server) = server(vec![
            (200, json!({"items":[original()]})),
            (200, json!({"id":"v1"})),
            (200, json!({"items":[actual]})),
        ])
        .await;
        let result = api.update(&edit()).await.unwrap();
        assert_eq!(result.privacy_status, "private");
        assert_eq!(result.etag, "rev2");
        let requests = server.await.unwrap();
        assert!(requests[1].starts_with("PUT /videos?"));
        assert!(requests[1].contains("if-match: rev1"));
        let body: Value =
            serde_json::from_str(requests[1].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["snippet"]["tags"], json!(["短剧"]));
        assert_eq!(body["status"]["privacyStatus"], "public");
    }
    #[tokio::test]
    async fn repeated_join_does_not_duplicate_playlist_item() {
        let (api, server) = server(vec![
            (200, json!({"items":[original()]})),
            (200, json!({"items":[{"snippet":{"channelId":"c1"}}]})),
            (200, json!({"items":[{"id":"member1"}]})),
        ])
        .await;
        api.set_membership("v1", "pl1", true).await.unwrap();
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests.iter().all(|r| r.starts_with("GET")));
    }
    #[tokio::test]
    async fn removal_only_deletes_memberships_of_target_video() {
        let (api, server) = server(vec![
            (200, json!({"items":[original()]})),
            (200, json!({"items":[{"snippet":{"channelId":"c1"}}]})),
            (200, json!({"items":[{"id":"member1"}]})),
            (204, Value::Null),
        ])
        .await;
        api.set_membership("v1", "pl1", false).await.unwrap();
        let requests = server.await.unwrap();
        assert!(requests[2].contains("videoId=v1"));
        assert!(requests[3].starts_with("DELETE /playlistItems?id=member1 "));
    }
    #[tokio::test]
    async fn insufficient_scope_returns_actionable_error_without_remote_body() {
        let (api, server) = server(vec![(403,json!({"error":{"message":"sensitive-remote-body","errors":[{"reason":"insufficientPermissions"}]}}))]).await;
        let error = api.list("").await.err().unwrap();
        assert_eq!(error.code, "AUTH_REQUIRED");
        assert!(!error.message.contains("sensitive"));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn playlist_discovery_follows_all_pages() {
        let (api, server) = server(vec![(200,channel()),(200,json!({"items":[original()]})),
            (200,json!({"items":[{"id":"pl1","snippet":{"channelId":"c1","title":"第一清单"},"status":{"privacyStatus":"private"}}],"nextPageToken":"next"})),
            (200,json!({"items":[{"id":"pl2","snippet":{"channelId":"c1","title":"第二清单"},"status":{"privacyStatus":"public"}}]})),
            (200,json!({"items":[]})),(200,json!({"items":[{"id":"member2"}]}))]).await;
        let rows = api.playlists("v1").await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].item_ids, vec!["member2"]);
        let requests = server.await.unwrap();
        assert!(requests[3].contains("pageToken=next"));
    }
}
