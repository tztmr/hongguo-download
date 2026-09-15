use super::{
    config::{import_private, OAuthClientConfig},
    duplicates::{self, DuplicateMatch, DuplicateQuery, KnownVideo},
    models::{
        AccountSummary, CredentialSummary, ThumbnailState, UploadIntent, YouTubeJob,
        YouTubeJobStatus, YouTubeSnapshot,
    },
    oauth::OAuthService,
    state::YouTubeStateStore,
    subtitles::{self, SubtitleRequest, SubtitleState},
    thumbnail::{prepare_thumbnail, set_thumbnail},
    upload::{RefreshCallback, ResumableUploader, UploadCancellationToken, UploadProgressEvent},
    vault::{OsTokenVault, TokenVault},
};
use crate::{
    media::{MediaJobService, MediaTools},
    platform_fs::{replace_file, sync_directory},
    AppError,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const MAX_CONCURRENT_UPLOADS: usize = 5;

pub trait YouTubeEventSink: Send + Sync + 'static {
    fn emit(&self, job: YouTubeJob);
}

#[derive(Default)]
pub struct NullYouTubeEventSink;

impl YouTubeEventSink for NullYouTubeEventSink {
    fn emit(&self, _job: YouTubeJob) {}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredUpload {
    job: YouTubeJob,
    intent: UploadIntent,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StoredUploads {
    version: u32,
    items: Vec<StoredUpload>,
}

pub struct YouTubeService {
    config_dir: PathBuf,
    data_dir: PathBuf,
    state: Arc<YouTubeStateStore>,
    vault: Arc<dyn TokenVault>,
    uploads: Mutex<Vec<StoredUpload>>,
    history_lock: Mutex<()>,
    running: Mutex<HashMap<String, UploadCancellationToken>>,
    media_jobs: Arc<MediaJobService>,
    event_sink: Arc<dyn YouTubeEventSink>,
}

impl YouTubeService {
    pub fn load(
        config_dir: PathBuf,
        data_dir: PathBuf,
        media_jobs: Arc<MediaJobService>,
        event_sink: Arc<dyn YouTubeEventSink>,
    ) -> Result<Arc<Self>, AppError> {
        let state = Arc::new(YouTubeStateStore::load(&config_dir)?);
        let jobs_path = data_dir.join("youtube/uploads.json");
        let mut uploads = match fs::read(jobs_path) {
            Ok(bytes) => serde_json::from_slice::<StoredUploads>(&bytes)
                .ok()
                .filter(|value| value.version == 1)
                .map(|value| value.items)
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        if restore_upload_queue(&mut uploads) {
            persist_uploads(&data_dir, &uploads)?;
        }
        let service = Arc::new(Self {
            config_dir,
            data_dir,
            state,
            vault: Arc::new(OsTokenVault),
            uploads: Mutex::new(uploads),
            history_lock: Mutex::new(()),
            running: Mutex::new(HashMap::new()),
            media_jobs,
            event_sink,
        });
        service.dispatch_uploads()?;
        Ok(service)
    }

    pub fn snapshot(&self) -> YouTubeSnapshot {
        let mut snapshot = self.state.snapshot();
        snapshot.jobs = self
            .uploads
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .map(|item| item.job.clone())
            .collect();
        snapshot
    }

    pub fn import_credential(&self, source: &Path) -> Result<CredentialSummary, AppError> {
        if !self.running.lock().map_err(state_lock_error)?.is_empty() {
            return Err(AppError::new(
                "UPLOAD_RUNNING",
                "上传运行中不能替换 OAuth 凭证",
            ));
        }
        let imported = import_private(source, &self.config_dir)?;
        self.state.set_credential(imported.summary.clone())?;
        Ok(imported.summary)
    }

    pub async fn authorize(
        &self,
        on_callback: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<AccountSummary, AppError> {
        let service = self.oauth_service()?.with_callback_received(on_callback);
        let account = service.authorize().await?;
        self.state.upsert_channel(account.clone())?;
        Ok(account)
    }

    pub fn set_active_channel(&self, channel_id: &str) -> Result<YouTubeSnapshot, AppError> {
        self.state.set_active_channel(channel_id)?;
        Ok(self.snapshot())
    }

    pub async fn revoke(&self, channel_id: &str) -> Result<YouTubeSnapshot, AppError> {
        if !self.running.lock().map_err(state_lock_error)?.is_empty() {
            return Err(AppError::new("UPLOAD_RUNNING", "上传运行中不能撤销授权"));
        }
        self.oauth_service()?.revoke(channel_id).await?;
        self.state.remove_channel(channel_id)?;
        Ok(self.snapshot())
    }

    pub fn remove_credential(&self) -> Result<YouTubeSnapshot, AppError> {
        if !self.running.lock().map_err(state_lock_error)?.is_empty() {
            return Err(AppError::new(
                "UPLOAD_RUNNING",
                "上传运行中不能删除 OAuth 凭证",
            ));
        }
        let path = self.credential_path();
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(service_io(error)),
        }
        self.state.clear_credential()?;
        Ok(self.snapshot())
    }

    pub(super) async fn management_api(
        &self,
        channel_id: &str,
    ) -> Result<super::management::ManagementApi, AppError> {
        self.ensure_check_channel(channel_id)?;
        let token = self.oauth_service()?.access_token(channel_id).await?;
        self.ensure_check_channel(channel_id)?;
        super::management::ManagementApi::new(channel_id, token)
    }

    async fn analytics_api(
        &self,
        channel_id: &str,
    ) -> Result<super::analytics::AnalyticsApi, AppError> {
        self.ensure_check_channel(channel_id)?;
        let token = self.oauth_service()?.access_token(channel_id).await?;
        self.ensure_check_channel(channel_id)?;
        super::analytics::AnalyticsApi::new(channel_id, token)
    }

    pub async fn channel_analytics_snapshot(
        &self,
        channel_id: &str,
    ) -> Result<super::analytics::ChannelAnalyticsSnapshot, AppError> {
        self.analytics_api(channel_id).await?.snapshot().await
    }

    pub async fn channel_analytics_report(
        &self,
        channel_id: &str,
        start_date: &str,
        end_date: &str,
        video_id: Option<&str>,
    ) -> Result<super::analytics::AnalyticsReport, AppError> {
        self.analytics_api(channel_id)
            .await?
            .report_range(start_date, end_date, video_id)
            .await
    }

    pub async fn channel_analytics_breakdown(
        &self,
        channel_id: &str,
        start_date: &str,
        end_date: &str,
        kind: super::analytics::BreakdownKind,
        video_id: Option<&str>,
    ) -> Result<super::analytics::AnalyticsBreakdown, AppError> {
        self.analytics_api(channel_id)
            .await?
            .breakdown(start_date, end_date, kind, video_id)
            .await
    }

    pub async fn channel_video(
        &self,
        channel_id: &str,
        video_id: &str,
    ) -> Result<super::management::ManagedVideo, AppError> {
        let mut video = self
            .management_api(channel_id)
            .await?
            .detail(video_id)
            .await?;
        self.annotate_video_formats(channel_id, std::slice::from_mut(&mut video));
        Ok(video)
    }

    pub async fn list_channel_videos(
        &self,
        channel_id: &str,
        page_token: &str,
    ) -> Result<super::management::VideoPage, AppError> {
        let mut page = self
            .management_api(channel_id)
            .await?
            .list(page_token)
            .await?;
        self.annotate_video_formats(channel_id, &mut page.items);
        Ok(page)
    }
    // Local format evidence survives clearing the completed upload queue. It is
    // supplementary: unavailable history must not hide remotely listed videos.
    fn annotate_video_formats(
        &self,
        channel_id: &str,
        videos: &mut [super::management::ManagedVideo],
    ) {
        let mut known = {
            let _guard = self
                .history_lock
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            duplicates::read_history(&self.data_dir).unwrap_or_default()
        };
        known.extend(
            self.uploads
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .iter()
                .filter_map(|stored| {
                    stored
                        .job
                        .video_id
                        .as_deref()
                        .map(|id| known_video(stored, id))
                }),
        );
        super::management::apply_upload_formats(channel_id, videos, &known);
    }
    pub async fn lookup_channel_videos(
        &self,
        channel_id: &str,
        video_ids: &[String],
    ) -> Result<super::management::VideoLookup, AppError> {
        let mut result = self
            .management_api(channel_id)
            .await?
            .lookup(video_ids)
            .await?;
        self.annotate_video_formats(channel_id, &mut result.items);
        Ok(result)
    }
    pub async fn delete_channel_video(
        &self,
        channel_id: &str,
        video_id: &str,
    ) -> Result<(), AppError> {
        self.management_api(channel_id)
            .await?
            .delete_video(video_id)
            .await
    }
    pub async fn update_channel_video(
        &self,
        request: &super::management::VideoUpdate,
    ) -> Result<super::management::ManagedVideo, AppError> {
        let mut video = self
            .management_api(&request.channel_id)
            .await?
            .update(request)
            .await?;
        self.annotate_video_formats(&request.channel_id, std::slice::from_mut(&mut video));
        // Keep local upload rows consistent with the confirmed remote metadata.
        let mut uploads = self.uploads.lock().map_err(state_lock_error)?;
        for stored in uploads.iter_mut().filter(|u| {
            u.job.channel_id == request.channel_id
                && u.job.video_id.as_deref() == Some(&request.video_id)
        }) {
            stored.job.title = video.title.clone();
            stored.intent.title = video.title.clone();
            stored.intent.description = video.description.clone();
            let privacy = serde_json::from_value(serde_json::json!(video.privacy_status))
                .map_err(|_| AppError::new("YOUTUBE_MANAGEMENT_RESPONSE", "YouTube 可见性无效"))?;
            stored.job.actual_privacy_status = Some(privacy);
            stored.intent.privacy_status = privacy;
            self.event_sink.emit(stored.job.clone());
        }
        persist_uploads(&self.data_dir, &uploads)?;
        Ok(video)
    }
    pub async fn channel_video_playlists(
        &self,
        channel_id: &str,
        video_id: &str,
    ) -> Result<Vec<super::management::ManagedPlaylist>, AppError> {
        self.management_api(channel_id)
            .await?
            .playlists(video_id)
            .await
    }
    pub async fn set_channel_video_playlist(
        &self,
        channel_id: &str,
        video_id: &str,
        playlist_id: &str,
        included: bool,
    ) -> Result<(), AppError> {
        self.management_api(channel_id)
            .await?
            .set_membership(video_id, playlist_id, included)
            .await
    }
    pub async fn create_channel_playlist(
        &self,
        channel_id: &str,
        title: &str,
        privacy: &str,
    ) -> Result<super::management::ManagedPlaylist, AppError> {
        self.management_api(channel_id)
            .await?
            .create_playlist(title, privacy)
            .await
    }
    pub async fn set_channel_video_thumbnail(
        &self,
        channel_id: &str,
        video_id: &str,
        path: &Path,
    ) -> Result<(), AppError> {
        self.management_api(channel_id)
            .await?
            .video(video_id)
            .await?;
        let temp = ThumbnailWorkspace::create(&self.data_dir)?;
        let executable = std::env::current_exe().map_err(service_io)?;
        let tools = executable
            .parent()
            .ok_or_else(|| AppError::new("MEDIA_TOOL_MISSING", "无法定位媒体工具"))
            .and_then(MediaTools::from_resource_root)?;
        let prepared = prepare_thumbnail(path, &tools, &temp.0)?;
        let token = self.oauth_service()?.access_token(channel_id).await?;
        self.ensure_check_channel(channel_id)?;
        set_thumbnail(&reqwest::Client::new(), video_id, &prepared, &token).await
    }

    pub async fn check_upload(
        &self,
        query: &DuplicateQuery,
    ) -> Result<Vec<DuplicateMatch>, AppError> {
        if query.title.trim().is_empty() || query.title.chars().count() > 100 {
            return Err(AppError::new(
                "UPLOAD_REQUEST_INVALID",
                "请填写有效的上传标题",
            ));
        }
        if query.season.is_some_and(|n| n == 0 || n > 999) {
            return Err(AppError::new(
                "UPLOAD_SEASON_INVALID",
                "季数须为 1～999，留空则自动识别",
            ));
        }
        let mut query = query.clone();
        if let Some(path) = query.source_path.clone() {
            let requested = query.upload_format;
            query.upload_format = tokio::task::spawn_blocking(move || {
                if requested == super::format::UploadFormat::Auto {
                    super::format::detect_file(&path)
                } else {
                    super::format::validate_file(requested, &path)?;
                    Ok(requested)
                }
            })
            .await
            .map_err(|_| AppError::new("UPLOAD_FORMAT_PROBE_FAILED", "无法检查上传视频"))??;
        }
        self.ensure_check_channel(&query.channel_id)?;
        let token = self
            .oauth_service()?
            .access_token(&query.channel_id)
            .await?;
        let mut videos = duplicates::channel_videos(&query.channel_id, &token).await?;
        // Include local records while YouTube's upload list is still catching up.
        let uploads = self.uploads.lock().map_err(state_lock_error)?;
        for item in uploads.iter() {
            if let Some(video_id) = &item.job.video_id {
                videos.push(known_video(item, video_id));
            } else if is_active(item.job.status) || item.job.status == YouTubeJobStatus::Paused {
                videos.push(known_video(item, &format!("queued:{}", item.job.id)));
            }
        }
        drop(uploads);
        let _history_guard = self.history_lock.lock().map_err(state_lock_error)?;
        videos.extend(duplicates::read_history(&self.data_dir)?);
        self.ensure_check_channel(&query.channel_id)?;
        Ok(duplicates::find_matches(&query, &videos))
    }

    fn ensure_check_channel(&self, channel_id: &str) -> Result<(), AppError> {
        if self.state.snapshot().active_channel_id.as_deref() != Some(channel_id) {
            return Err(AppError::new(
                "YOUTUBE_CHANNEL_CHANGED",
                "所选频道已变化，请重新检查后上传",
            ));
        }
        Ok(())
    }

    fn remember_upload(&self, stored: &StoredUpload, video_id: &str) -> Result<(), AppError> {
        let _guard = self.history_lock.lock().map_err(state_lock_error)?;
        duplicates::remember_video(&self.data_dir, known_video(stored, video_id))
    }

    pub async fn start_upload(
        self: &Arc<Self>,
        mut intent: UploadIntent,
    ) -> Result<YouTubeJob, AppError> {
        if let Some(subtitle) = &mut intent.subtitle {
            if subtitle.path.is_none() {
                subtitle.path = self.find_subtitle(&intent.file_path);
                if subtitle.path.is_none() {
                    intent.subtitle = None;
                }
            }
        }
        intent.validate()?;
        if intent.upload_format == super::format::UploadFormat::Auto {
            let path = intent.file_path.clone();
            intent.upload_format =
                tokio::task::spawn_blocking(move || super::format::detect_file(&path))
                    .await
                    .map_err(|_| {
                        AppError::new("UPLOAD_FORMAT_PROBE_FAILED", "无法检查上传视频")
                    })??;
        }
        if let Some(identity) = &mut intent.dedup {
            identity.season =
                duplicates::inferred_season(identity.season, &identity.drama_title, &intent.title);
        }
        let identity = intent.dedup.as_ref().ok_or_else(|| {
            AppError::new("YOUTUBE_CHECK_REQUIRED", "请重新打开上传窗口完成频道查重")
        })?;
        let query = DuplicateQuery {
            channel_id: identity.channel_id.clone(),
            title: intent.title.clone(),
            book_id: identity.book_id.clone(),
            drama_title: identity.drama_title.clone(),
            season: identity.season,
            upload_format: intent.upload_format,
            source_path: None,
        };
        let matches = self.check_upload(&query).await?;
        if (!identity.allow_duplicate && !matches.is_empty())
            || (intent.job_id.starts_with("auto-")
                && matches
                    .iter()
                    .any(|m| m.confidence == duplicates::MatchConfidence::Confirmed))
        {
            return Err(AppError::new(
                "YOUTUBE_DUPLICATE_FOUND",
                "发现重复或身份信息不完整的影片，请重新查重并核对季数与视频类型",
            ));
        }
        self.ensure_check_channel(&identity.channel_id)?;
        if !self
            .media_jobs
            .is_validated_upload_source(&intent.file_path)
        {
            return Err(AppError::new(
                "UPLOAD_SOURCE_NOT_MERGED",
                "只能上传由应用验证完成的合并视频",
            ));
        }
        let channel_id = identity.channel_id.clone();
        let total = fs::metadata(&intent.file_path).map_err(service_io)?.len();
        let job = YouTubeJob {
            id: intent.job_id.clone(),
            title: intent.title.clone(),
            channel_id,
            source_path: fs::canonicalize(&intent.file_path).map_err(service_io)?,
            status: YouTubeJobStatus::Queued,
            uploaded_bytes: 0,
            total_bytes: total,
            percent: 0.0,
            error_code: None,
            error_message: None,
            video_id: None,
            youtube_url: None,
            actual_privacy_status: None,
            thumbnail_state: ThumbnailState::Pending,
            subtitle_state: if intent.subtitle.is_some() {
                super::subtitles::SubtitleState::Pending
            } else {
                super::subtitles::SubtitleState::Skipped
            },
            subtitle_error: None,
            completion_notified_at: None,
            failure_notified_at: None,
        };
        {
            let mut uploads = self.uploads.lock().map_err(state_lock_error)?;
            if uploads.iter().any(|item| item.job.id == job.id) {
                return Err(AppError::new(
                    "UPLOAD_JOB_EXISTS",
                    "YouTube 上传任务 ID 已存在",
                ));
            }
            ensure_upload_not_duplicate(&uploads, &job)?;
            ensure_no_local_duplicate(&uploads, &intent, &job)?;
            uploads.push(StoredUpload {
                job: job.clone(),
                intent,
            });
            persist_uploads(&self.data_dir, &uploads)?;
        }
        self.event_sink.emit(job.clone());
        self.dispatch_uploads()?;
        Ok(job)
    }

    pub fn pause_upload(&self, job_id: &str) -> Result<YouTubeJob, AppError> {
        let running = self.running.lock().map_err(state_lock_error)?;
        let token = running.get(job_id);
        self.update_job(job_id, |job| {
            pause_upload_job(job, token.is_some())?;
            if let Some(token) = token {
                token.pause();
            }
            Ok(())
        })
    }

    pub fn resume_upload(self: &Arc<Self>, job_id: &str) -> Result<YouTubeJob, AppError> {
        self.requeue_upload(job_id, true)
    }

    pub fn retry_upload(self: &Arc<Self>, job_id: &str) -> Result<YouTubeJob, AppError> {
        self.requeue_upload(job_id, false)
    }

    fn requeue_upload(
        self: &Arc<Self>,
        job_id: &str,
        resume: bool,
    ) -> Result<YouTubeJob, AppError> {
        let job = {
            let mut uploads = self.uploads.lock().map_err(state_lock_error)?;
            let index = uploads
                .iter()
                .position(|item| item.job.id == job_id)
                .ok_or_else(|| AppError::new("UPLOAD_JOB_NOT_FOUND", "YouTube 上传任务不存在"))?;
            let mut job = uploads[index].job.clone();
            let allowed = if resume {
                job.status == YouTubeJobStatus::Paused
            } else {
                matches!(
                    job.status,
                    YouTubeJobStatus::Failed | YouTubeJobStatus::Cancelled
                )
            };
            if !allowed {
                return Err(AppError::new(
                    "UPLOAD_INVALID_TRANSITION",
                    "当前 YouTube 任务不能重试",
                ));
            }
            ensure_upload_not_duplicate(&uploads, &job)?;
            job.status = YouTubeJobStatus::Queued;
            job.error_code = None;
            job.error_message = None;
            job.completion_notified_at = None;
            job.failure_notified_at = None;
            uploads[index].job = job.clone();
            persist_uploads(&self.data_dir, &uploads)?;
            job
        };
        self.event_sink.emit(job.clone());
        self.dispatch_uploads()?;
        Ok(job)
    }

    pub fn cancel_upload(&self, job_id: &str) -> Result<(), AppError> {
        // Use the same lock order as dispatch so a queued cancellation cannot
        // race with reserving a worker slot.
        let running = self.running.lock().map_err(state_lock_error)?;
        if let Some(token) = running.get(job_id) {
            token.cancel();
            self.update_job(job_id, |job| {
                if job.status == YouTubeJobStatus::Paused {
                    job.status = YouTubeJobStatus::Cancelled;
                }
                Ok(())
            })?;
        } else {
            self.update_job(job_id, cancel_queued_upload)?;
        }
        Ok(())
    }

    pub fn delete_upload(&self, job_id: &str) -> Result<(), AppError> {
        // Match dispatch's running -> uploads lock order so deletion cannot
        // race with a queued job being assigned to a worker.
        let running = self.running.lock().map_err(state_lock_error)?;
        let mut uploads = self.uploads.lock().map_err(state_lock_error)?;
        let (next, removed_id) = uploads_without_job(&uploads, job_id)?;
        if let Some(stored) = uploads.iter().find(|item| item.job.id == job_id) {
            if let Some(video_id) = &stored.job.video_id {
                self.remember_upload(stored, video_id)?;
            }
        }
        persist_uploads(&self.data_dir, &next)?;
        *uploads = next;
        if let Some(token) = running.get(job_id) {
            token.cancel();
        }
        drop(uploads);
        drop(running);
        let checkpoint = self
            .data_dir
            .join("youtube/uploads")
            .join(format!("{removed_id}.json"));
        let _ = fs::remove_file(checkpoint);
        Ok(())
    }

    pub fn find_subtitle(&self, source: &Path) -> Option<PathBuf> {
        subtitles::find_subtitle(&self.media_jobs.snapshot().jobs, source)
    }

    /// Attach to an existing video. None retries the stored subtitle without uploading video bytes.
    pub async fn upload_subtitle(
        &self,
        job_id: &str,
        request: Option<SubtitleRequest>,
    ) -> Result<YouTubeJob, AppError> {
        let stored = {
            let mut uploads = self.uploads.lock().map_err(state_lock_error)?;
            let item = uploads
                .iter_mut()
                .find(|item| item.job.id == job_id)
                .ok_or_else(|| AppError::new("UPLOAD_JOB_NOT_FOUND", "YouTube 上传任务不存在"))?;
            if item.job.video_id.is_none() || is_active(item.job.status) {
                return Err(AppError::new(
                    "UPLOAD_INVALID_TRANSITION",
                    "请等待视频上传完成后再上传字幕",
                ));
            }
            let mut subtitle = request
                .or_else(|| item.intent.subtitle.clone())
                .ok_or_else(|| AppError::new("SUBTITLE_MISSING", "请先选择字幕文件"))?;
            if subtitle.path.is_none() {
                subtitle.path = self.find_subtitle(&item.job.source_path);
            }
            if subtitle.path.is_none() {
                return Err(AppError::new(
                    "SUBTITLE_MISSING",
                    "没有找到匹配的整季字幕，请先提取字幕或手动选择 SRT",
                ));
            }
            subtitle.validate()?;
            item.intent.subtitle = Some(subtitle);
            item.job.status = YouTubeJobStatus::UploadingSubtitles;
            item.job.subtitle_state = SubtitleState::Pending;
            item.job.subtitle_error = None;
            item.job.failure_notified_at = None;
            item.job.completion_notified_at = None;
            let stored = item.clone();
            persist_uploads(&self.data_dir, &uploads)?;
            stored
        };
        self.event_sink.emit(stored.job.clone());
        let result = match self.oauth_service() {
            Ok(oauth) => {
                self.publish_subtitle(&stored, &oauth, stored.job.video_id.as_deref().unwrap())
                    .await
            }
            Err(error) => Err(error),
        };
        self.update_job(job_id, |job| {
            apply_subtitle_result(job, result);
            Ok(())
        })
    }

    async fn publish_subtitle(
        &self,
        stored: &StoredUpload,
        oauth: &OAuthService,
        video_id: &str,
    ) -> Result<SubtitleState, AppError> {
        let Some(request) = &stored.intent.subtitle else {
            return Ok(SubtitleState::Skipped);
        };
        self.update_job(&stored.job.id, |job| {
            job.status = YouTubeJobStatus::UploadingSubtitles;
            Ok(())
        })?;
        let token = oauth.access_token(&stored.job.channel_id).await?;
        subtitles::upload_subtitle(video_id, request, &token).await?;
        Ok(SubtitleState::Submitted)
    }

    pub async fn retry_thumbnail(&self, job_id: &str) -> Result<YouTubeJob, AppError> {
        let stored = self
            .uploads
            .lock()
            .map_err(state_lock_error)?
            .iter()
            .find(|item| item.job.id == job_id)
            .cloned()
            .ok_or_else(|| AppError::new("UPLOAD_JOB_NOT_FOUND", "YouTube 上传任务不存在"))?;
        if stored.job.thumbnail_state != ThumbnailState::Failed || is_active(stored.job.status) {
            return Err(AppError::new(
                "UPLOAD_INVALID_TRANSITION",
                "当前 YouTube 任务不能重试封面",
            ));
        }
        let video_id = stored
            .job
            .video_id
            .clone()
            .ok_or_else(|| AppError::new("UPLOAD_VIDEO_ID_MISSING", "YouTube 视频 ID 缺失"))?;
        if stored.intent.cover_path.is_none() {
            return Err(AppError::new("UPLOAD_COVER_INVALID", "上传封面无效"));
        }
        let oauth = self.oauth_service()?;
        self.update_job(job_id, |job| {
            if is_active(job.status) {
                return Err(AppError::new(
                    "UPLOAD_INVALID_TRANSITION",
                    "上传任务正在处理，请稍后重试封面",
                ));
            }
            job.status = YouTubeJobStatus::SettingThumbnail;
            Ok(())
        })?;
        match self.publish_thumbnail(&stored, &oauth, &video_id).await {
            Ok(state) => self.update_job(job_id, |job| {
                job.thumbnail_state = state;
                job.status = attachment_status(job);
                job.error_code = None;
                job.error_message = None;
                Ok(())
            }),
            Err(error) => self.update_job(job_id, |job| {
                job.thumbnail_state = ThumbnailState::Failed;
                job.status = attachment_status(job);
                job.error_code = Some(error.code);
                job.error_message = Some(error.message);
                Ok(())
            }),
        }
    }

    pub fn mark_notified(&self, job_id: &str, succeeded: bool) -> Result<YouTubeJob, AppError> {
        self.update_job(job_id, |job| {
            let terminal_matches = if succeeded {
                job.status == YouTubeJobStatus::Completed
            } else {
                matches!(
                    job.status,
                    YouTubeJobStatus::Failed
                        | YouTubeJobStatus::VideoUploadedThumbnailFailed
                        | YouTubeJobStatus::VideoUploadedSubtitleFailed
                )
            };
            if !terminal_matches {
                return Err(AppError::new(
                    "UPLOAD_INVALID_TRANSITION",
                    "YouTube 上传任务尚未结束",
                ));
            }
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            if succeeded {
                job.completion_notified_at.get_or_insert(timestamp);
            } else {
                job.failure_notified_at.get_or_insert(timestamp);
            }
            Ok(())
        })
    }

    fn dispatch_uploads(self: &Arc<Self>) -> Result<(), AppError> {
        let reserved = {
            let mut running = self.running.lock().map_err(state_lock_error)?;
            let uploads = self.uploads.lock().map_err(state_lock_error)?;
            reserve_uploads(&uploads, &mut running)
        };
        for (job_id, cancellation) in reserved {
            let service = self.clone();
            tauri::async_runtime::spawn(async move {
                service
                    .clone()
                    .run_upload(job_id.clone(), cancellation)
                    .await;
                if let Ok(mut running) = service.running.lock() {
                    running.remove(&job_id);
                }
                // Success, failure and cancellation all release a slot. A retry
                // queued during worker teardown is picked up here as well.
                let _ = service.dispatch_uploads();
            });
        }
        Ok(())
    }

    async fn run_upload(self: Arc<Self>, job_id: String, cancellation: UploadCancellationToken) {
        let stored = self
            .uploads
            .lock()
            .ok()
            .and_then(|items| items.iter().find(|item| item.job.id == job_id).cloned());
        let Some(stored) = stored else { return };
        if cancellation.is_cancelled() || cancellation.is_paused() {
            let _ = self.update_job(&job_id, |job| {
                job.status = if cancellation.is_paused() {
                    YouTubeJobStatus::Paused
                } else {
                    YouTubeJobStatus::Cancelled
                };
                Ok(())
            });
            return;
        }
        let _ = self.update_job(&job_id, |job| {
            if job.status != YouTubeJobStatus::Pausing {
                job.status = YouTubeJobStatus::PreparingAuthorization;
            }
            Ok(())
        });
        let oauth = match self.oauth_service() {
            Ok(value) => Arc::new(value),
            Err(error) => {
                let _ = self.fail_job(&job_id, error, &cancellation);
                return;
            }
        };
        let token_result = cancellation
            .interruptible(oauth.access_token(&stored.job.channel_id))
            .await;
        let token = match token_result.and_then(|result| result) {
            Ok(value) => value,
            Err(error) => {
                let _ = self.fail_job(&job_id, error, &cancellation);
                return;
            }
        };
        let refresh_oauth = oauth.clone();
        let refresh_channel = stored.job.channel_id.clone();
        let refresh: RefreshCallback = Arc::new(move || {
            let oauth = refresh_oauth.clone();
            let channel = refresh_channel.clone();
            Box::pin(async move { oauth.access_token(&channel).await })
        });
        let progress_service = self.clone();
        let progress_job_id = job_id.clone();
        let progress: Arc<dyn Fn(UploadProgressEvent) + Send + Sync> = Arc::new(move |event| {
            let _ = progress_service.update_job(&progress_job_id, |job| {
                if job.status != YouTubeJobStatus::Pausing {
                    job.status = event.status;
                }
                job.uploaded_bytes = event.uploaded_bytes;
                job.total_bytes = event.total_bytes;
                job.percent = event.percent;
                Ok(())
            });
        });
        let uploader = ResumableUploader::new(self.data_dir.join("youtube/uploads"));
        let result = uploader
            .upload(
                &stored.intent,
                &stored.job.channel_id,
                token,
                refresh,
                cancellation.clone(),
                progress,
            )
            .await;
        match result {
            Ok(result) => {
                // Record success before thumbnail work; even deleted queue rows retain identity.
                let history_error = self.remember_upload(&stored, &result.video_id).err();
                let _ = self.update_job(&job_id, |job| {
                    job.video_id = Some(result.video_id.clone());
                    job.youtube_url = Some(result.youtube_url.clone());
                    job.actual_privacy_status = result.privacy_status;
                    Ok(())
                });
                let thumbnail_state = self
                    .publish_thumbnail(&stored, &oauth, &result.video_id)
                    .await;
                let _ = self.update_job(&job_id, |job| {
                    job.video_id = Some(result.video_id.clone());
                    job.youtube_url = Some(result.youtube_url.clone());
                    job.actual_privacy_status = result.privacy_status;
                    match thumbnail_state {
                        Ok(state) => {
                            job.thumbnail_state = state;
                            job.status = YouTubeJobStatus::Completed;
                            job.percent = 100.0;
                            if let Some(error) = &history_error {
                                job.error_code = Some(error.code.clone());
                                job.error_message =
                                    Some(format!("视频已上传，但{}", error.message));
                            }
                        }
                        Err(ref error) => {
                            job.thumbnail_state = ThumbnailState::Failed;
                            job.status = YouTubeJobStatus::VideoUploadedThumbnailFailed;
                            job.percent = 100.0;
                            job.error_code = Some(error.code.clone());
                            job.error_message = Some(error.message.clone());
                        }
                    }
                    // Do not emit a completed notification before subtitles have been submitted.
                    if stored.intent.subtitle.is_some() {
                        job.status = YouTubeJobStatus::UploadingSubtitles;
                    }
                    Ok(())
                });
                let subtitle_result = self
                    .publish_subtitle(&stored, &oauth, &result.video_id)
                    .await;
                let _ = self.update_job(&job_id, |job| {
                    apply_subtitle_result(job, subtitle_result);
                    Ok(())
                });
            }
            Err(error) => {
                let _ = self.fail_job(&job_id, error, &cancellation);
            }
        }
    }

    async fn publish_thumbnail(
        &self,
        stored: &StoredUpload,
        oauth: &OAuthService,
        video_id: &str,
    ) -> Result<ThumbnailState, AppError> {
        let Some(cover) = &stored.intent.cover_path else {
            return Ok(ThumbnailState::Skipped);
        };
        self.update_job(&stored.job.id, |job| {
            job.status = YouTubeJobStatus::SettingThumbnail;
            Ok(())
        })?;
        let temp = ThumbnailWorkspace::create(&self.data_dir)?;
        let executable = std::env::current_exe().map_err(service_io)?;
        let tools = executable
            .parent()
            .ok_or_else(|| AppError::new("MEDIA_TOOL_MISSING", "无法定位媒体工具"))
            .and_then(MediaTools::from_resource_root)?;
        let prepared = prepare_thumbnail(cover, &tools, &temp.0)?;
        let token = oauth.access_token(&stored.job.channel_id).await?;
        set_thumbnail(&reqwest::Client::new(), video_id, &prepared, &token).await?;
        let _ = fs::remove_file(prepared);
        Ok(ThumbnailState::Succeeded)
    }

    fn fail_job(
        &self,
        job_id: &str,
        error: AppError,
        cancellation: &UploadCancellationToken,
    ) -> Result<YouTubeJob, AppError> {
        self.update_job(job_id, |job| {
            match error.code.as_str() {
                _ if cancellation.is_cancelled() => job.status = YouTubeJobStatus::Cancelled,
                _ if cancellation.is_paused() => job.status = YouTubeJobStatus::Paused,
                "UPLOAD_PAUSED" => job.status = YouTubeJobStatus::Paused,
                "UPLOAD_CANCELLED" => job.status = YouTubeJobStatus::Cancelled,
                _ => {
                    job.status = YouTubeJobStatus::Failed;
                    job.error_code = Some(error.code);
                    job.error_message = Some(error.message);
                }
            }
            Ok(())
        })
    }

    fn update_job(
        &self,
        job_id: &str,
        apply: impl FnOnce(&mut YouTubeJob) -> Result<(), AppError>,
    ) -> Result<YouTubeJob, AppError> {
        let mut uploads = self.uploads.lock().map_err(state_lock_error)?;
        let item = uploads
            .iter_mut()
            .find(|item| item.job.id == job_id)
            .ok_or_else(|| AppError::new("UPLOAD_JOB_NOT_FOUND", "YouTube 上传任务不存在"))?;
        apply(&mut item.job)?;
        let job = item.job.clone();
        persist_uploads(&self.data_dir, &uploads)?;
        self.event_sink.emit(job.clone());
        Ok(job)
    }

    fn oauth_service(&self) -> Result<OAuthService, AppError> {
        let config = OAuthClientConfig::load(&self.credential_path())?;
        Ok(OAuthService::new(config, self.vault.clone()))
    }

    fn credential_path(&self) -> PathBuf {
        self.config_dir.join("youtube/oauth-client.json")
    }
}

struct ThumbnailWorkspace(PathBuf);

impl ThumbnailWorkspace {
    fn create(data_dir: &Path) -> Result<Self, AppError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = data_dir.join("youtube/thumbnail-temp");
        fs::create_dir_all(&root).map_err(service_io)?;
        loop {
            let path = root.join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(service_io(error)),
            }
        }
    }
}

impl Drop for ThumbnailWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn attachment_status(job: &YouTubeJob) -> YouTubeJobStatus {
    if job.subtitle_state == SubtitleState::Failed {
        YouTubeJobStatus::VideoUploadedSubtitleFailed
    } else if job.thumbnail_state == ThumbnailState::Failed {
        YouTubeJobStatus::VideoUploadedThumbnailFailed
    } else {
        YouTubeJobStatus::Completed
    }
}

fn apply_subtitle_result(job: &mut YouTubeJob, result: Result<SubtitleState, AppError>) {
    match result {
        Ok(state) => {
            job.subtitle_state = state;
            job.subtitle_error = None;
        }
        Err(error) => {
            job.subtitle_state = SubtitleState::Failed;
            job.subtitle_error = Some(error.message);
        }
    }
    job.status = attachment_status(job);
}

fn restore_upload_queue(uploads: &mut [StoredUpload]) -> bool {
    let mut changed = false;
    for item in uploads {
        if item.job.video_id.is_some() && is_active(item.job.status) {
            // Never re-upload a video after its ID has been persisted.
            item.job.percent = 100.0;
            item.job.uploaded_bytes = item.job.total_bytes;
            if item.intent.cover_path.is_some()
                && item.job.thumbnail_state != ThumbnailState::Succeeded
            {
                item.job.thumbnail_state = ThumbnailState::Failed;
                item.job.error_code = Some("THUMBNAIL_INTERRUPTED".into());
                item.job.error_message = Some("视频已上传，封面处理被中断，请仅重试封面".into());
            } else if item.intent.cover_path.is_none() {
                item.job.thumbnail_state = ThumbnailState::Skipped;
            }
            if item.intent.subtitle.is_some() && item.job.subtitle_state != SubtitleState::Submitted
            {
                item.job.subtitle_state = SubtitleState::Failed;
                item.job.subtitle_error = Some("视频已上传，字幕处理被中断，请仅重试字幕".into());
            }
            item.job.status = attachment_status(&item.job);
            changed = true;
        } else if item.job.status == YouTubeJobStatus::Pausing {
            item.job.status = YouTubeJobStatus::Paused;
            changed = true;
        } else if is_active(item.job.status) {
            // Automation owns restart policy; hold these jobs until its runner resumes them.
            item.job.status = if item.job.id.starts_with("auto-") {
                YouTubeJobStatus::Paused
            } else {
                YouTubeJobStatus::Queued
            };
            item.job.error_code = None;
            item.job.error_message = None;
            changed = true;
        }
    }
    changed
}

fn reserve_uploads(
    uploads: &[StoredUpload],
    running: &mut HashMap<String, UploadCancellationToken>,
) -> Vec<(String, UploadCancellationToken)> {
    let mut reserved = Vec::new();
    for item in uploads {
        if running.len() >= MAX_CONCURRENT_UPLOADS {
            break;
        }
        if item.job.status != YouTubeJobStatus::Queued || running.contains_key(&item.job.id) {
            continue;
        }
        let token = UploadCancellationToken::default();
        running.insert(item.job.id.clone(), token.clone());
        reserved.push((item.job.id.clone(), token));
    }
    reserved
}

fn known_video(stored: &StoredUpload, video_id: &str) -> KnownVideo {
    let identity = stored.intent.dedup.as_ref();
    let drama_title = identity.map(|i| i.drama_title.clone()).unwrap_or_default();
    KnownVideo {
        season: duplicates::inferred_season(
            identity.and_then(|i| i.season),
            &drama_title,
            &stored.job.title,
        ),
        drama_title,
        upload_format: stored.intent.upload_format,
        channel_id: stored.job.channel_id.clone(),
        video_id: video_id.into(),
        title: stored.job.title.clone(),
        book_id: stored
            .intent
            .dedup
            .as_ref()
            .map(|d| d.book_id.clone())
            .unwrap_or_default(),
    }
}

fn ensure_no_local_duplicate(
    uploads: &[StoredUpload],
    intent: &UploadIntent,
    job: &YouTubeJob,
) -> Result<(), AppError> {
    let Some(identity) = &intent.dedup else {
        return Ok(());
    };
    if identity.allow_duplicate {
        return Ok(());
    }
    let query = DuplicateQuery {
        channel_id: job.channel_id.clone(),
        title: intent.title.clone(),
        book_id: identity.book_id.clone(),
        drama_title: identity.drama_title.clone(),
        season: identity.season,
        upload_format: intent.upload_format,
        source_path: None,
    };
    let decisions: Vec<_> = uploads
        .iter()
        .filter(|item| {
            item.job.id != job.id
                && (is_active(item.job.status)
                    || item.job.status == YouTubeJobStatus::Paused
                    || item.job.video_id.is_some())
        })
        .filter_map(|item| duplicates::match_decision(&query, &known_video(item, "")))
        .collect();
    if decisions
        .iter()
        .any(|(_, confidence)| *confidence == duplicates::MatchConfidence::Confirmed)
    {
        return Err(AppError::new(
            "UPLOAD_DUPLICATE_QUEUED",
            "当前频道已有同一部剧、同季、同类型的上传任务",
        ));
    }
    if !decisions.is_empty() {
        return Err(AppError::new(
            "UPLOAD_DUPLICATE_REVIEW_REQUIRED",
            "队列中有疑似重复项，请重新查重并核对季数和视频类型",
        ));
    }
    Ok(())
}

fn ensure_upload_not_duplicate(uploads: &[StoredUpload], job: &YouTubeJob) -> Result<(), AppError> {
    if uploads.iter().any(|item| {
        item.job.id != job.id
            && (is_active(item.job.status) || item.job.status == YouTubeJobStatus::Paused)
            && item.job.channel_id == job.channel_id
            && item.job.source_path == job.source_path
    }) {
        return Err(AppError::new(
            "UPLOAD_DUPLICATE_ACTIVE",
            "此视频已在当前频道的上传队列中",
        ));
    }
    Ok(())
}

fn uploads_without_job(
    uploads: &[StoredUpload],
    job_id: &str,
) -> Result<(Vec<StoredUpload>, String), AppError> {
    let removed = uploads
        .iter()
        .find(|item| item.job.id == job_id)
        .ok_or_else(|| AppError::new("UPLOAD_JOB_NOT_FOUND", "YouTube 上传任务不存在"))?;
    Ok((
        uploads
            .iter()
            .filter(|item| item.job.id != job_id)
            .cloned()
            .collect(),
        removed.job.id.clone(),
    ))
}

fn pause_upload_job(job: &mut YouTubeJob, has_worker: bool) -> Result<(), AppError> {
    if matches!(
        job.status,
        YouTubeJobStatus::Paused | YouTubeJobStatus::Pausing
    ) {
        return Ok(());
    }
    if !matches!(
        job.status,
        YouTubeJobStatus::Queued
            | YouTubeJobStatus::PreparingAuthorization
            | YouTubeJobStatus::CreatingSession
            | YouTubeJobStatus::Uploading
            | YouTubeJobStatus::WaitingToRetry
    ) {
        return Err(AppError::new(
            "UPLOAD_INVALID_TRANSITION",
            "当前上传任务不能暂停",
        ));
    }
    job.status = if has_worker {
        YouTubeJobStatus::Pausing
    } else {
        YouTubeJobStatus::Paused
    };
    Ok(())
}

fn cancel_queued_upload(job: &mut YouTubeJob) -> Result<(), AppError> {
    if !matches!(
        job.status,
        YouTubeJobStatus::Queued | YouTubeJobStatus::Paused
    ) {
        return Err(AppError::new(
            "UPLOAD_NOT_RUNNING",
            "YouTube 上传任务未运行",
        ));
    }
    job.status = YouTubeJobStatus::Cancelled;
    Ok(())
}

fn is_active(status: YouTubeJobStatus) -> bool {
    matches!(
        status,
        YouTubeJobStatus::Queued
            | YouTubeJobStatus::Pausing
            | YouTubeJobStatus::PreparingAuthorization
            | YouTubeJobStatus::CreatingSession
            | YouTubeJobStatus::Uploading
            | YouTubeJobStatus::WaitingToRetry
            | YouTubeJobStatus::Processing
            | YouTubeJobStatus::SettingThumbnail
            | YouTubeJobStatus::UploadingSubtitles
    )
}

fn persist_uploads(data_dir: &Path, items: &[StoredUpload]) -> Result<(), AppError> {
    let directory = data_dir.join("youtube");
    fs::create_dir_all(&directory).map_err(service_io)?;
    let target = directory.join("uploads.json");
    let temporary = directory.join("uploads.json.tmp");
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(service_io)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(
        &mut writer,
        &StoredUploads {
            version: 1,
            items: items.to_vec(),
        },
    )
    .map_err(service_io)?;
    writer
        .flush()
        .and_then(|_| writer.get_ref().sync_all())
        .map_err(service_io)?;
    drop(writer);
    replace_file(&temporary, &target).map_err(service_io)?;
    sync_directory(&directory).map_err(service_io)
}

fn state_lock_error<T>(_error: std::sync::PoisonError<T>) -> AppError {
    AppError::new("YOUTUBE_STATE_ERROR", "YouTube 状态不可用")
}

fn service_io(error: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "YOUTUBE_IO",
        "YouTube 状态或文件操作失败",
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::super::models::PrivacyStatus;
    use super::*;

    fn queued(id: &str) -> StoredUpload {
        StoredUpload {
            job: YouTubeJob {
                id: id.into(),
                title: id.into(),
                channel_id: "channel".into(),
                source_path: PathBuf::from(format!("/{id}.mp4")),
                status: YouTubeJobStatus::Queued,
                uploaded_bytes: 0,
                total_bytes: 16,
                percent: 0.0,
                error_code: None,
                error_message: None,
                video_id: None,
                youtube_url: None,
                actual_privacy_status: None,
                thumbnail_state: ThumbnailState::Pending,
                subtitle_state: super::super::subtitles::SubtitleState::Skipped,
                subtitle_error: None,
                completion_notified_at: None,
                failure_notified_at: None,
            },
            intent: UploadIntent {
                upload_format: Default::default(),
                job_id: id.into(),
                dedup: None,
                file_path: PathBuf::from(format!("/{id}.mp4")),
                cover_path: None,
                subtitle: None,
                title: id.into(),
                description: String::new(),
                tags: vec![],
                category_id: "24".into(),
                privacy_status: PrivacyStatus::Private,
                self_declared_made_for_kids: false,
                contains_synthetic_media: false,
                has_paid_product_placement: false,
                audience_confirmed: true,
                synthetic_media_confirmed: true,
                publish_confirmed: true,
            },
        }
    }

    #[test]
    fn automatic_uploads_wait_for_runner_after_restart() {
        let mut uploads = vec![queued("auto-series-main"), queued("manual")];
        for item in &mut uploads {
            item.job.status = YouTubeJobStatus::Uploading;
        }
        assert!(restore_upload_queue(&mut uploads));
        assert_eq!(uploads[0].job.status, YouTubeJobStatus::Paused);
        assert_eq!(uploads[1].job.status, YouTubeJobStatus::Queued);
    }

    #[test]
    fn subtitle_failure_and_restart_never_requeue_an_uploaded_video() {
        let mut stored = queued("subtitle");
        stored.job.video_id = Some("existing-video".into());
        stored.job.thumbnail_state = ThumbnailState::Succeeded;
        stored.intent.cover_path = Some("cover.jpg".into());
        stored.intent.subtitle = Some(SubtitleRequest {
            path: Some("字幕.srt".into()),
            language: "zh-Hans".into(),
        });
        stored.job.status = YouTubeJobStatus::UploadingSubtitles;
        stored.job.subtitle_state = SubtitleState::Pending;
        let mut uploads = vec![stored];
        assert!(restore_upload_queue(&mut uploads));
        assert_eq!(
            uploads[0].job.status,
            YouTubeJobStatus::VideoUploadedSubtitleFailed
        );
        assert_eq!(uploads[0].job.thumbnail_state, ThumbnailState::Succeeded);
        assert_eq!(uploads[0].job.video_id.as_deref(), Some("existing-video"));
        assert!(reserve_uploads(&uploads, &mut HashMap::new()).is_empty());
        apply_subtitle_result(&mut uploads[0].job, Ok(SubtitleState::Submitted));
        assert_eq!(uploads[0].job.status, YouTubeJobStatus::Completed);
        uploads[0].job.thumbnail_state = ThumbnailState::Failed;
        apply_subtitle_result(&mut uploads[0].job, Ok(SubtitleState::Submitted));
        assert_eq!(
            uploads[0].job.status,
            YouTubeJobStatus::VideoUploadedThumbnailFailed
        );
    }

    #[test]
    fn old_saved_jobs_without_subtitle_fields_still_load_and_skip_subtitles() {
        let original = queued("legacy");
        let mut json = serde_json::to_value(&original).unwrap();
        json["job"].as_object_mut().unwrap().remove("subtitleState");
        json["job"].as_object_mut().unwrap().remove("subtitleError");
        let restored: StoredUpload = serde_json::from_value(json).unwrap();
        assert_eq!(restored.job.subtitle_state, SubtitleState::Skipped);
        assert!(restored.intent.subtitle.is_none());
    }

    #[test]
    fn queue_runs_five_and_fills_only_the_released_slot_in_order() {
        let mut jobs: Vec<_> = (0..6).map(|i| queued(&i.to_string())).collect();
        let mut running = HashMap::new();
        let first = reserve_uploads(&jobs, &mut running);
        assert_eq!(
            first.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            ["0", "1", "2", "3", "4"]
        );
        assert!(reserve_uploads(&jobs, &mut running).is_empty());
        jobs[1].job.status = YouTubeJobStatus::Completed;
        running.remove("1");
        let next = reserve_uploads(&jobs, &mut running);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].0, "5");
        assert_eq!(running.len(), 5);
    }

    #[test]
    fn queued_cancellation_is_skipped_and_terminal_jobs_cannot_be_cancelled() {
        let mut jobs: Vec<_> = (0..7).map(|i| queued(&i.to_string())).collect();
        let mut running = HashMap::new();
        reserve_uploads(&jobs, &mut running);
        cancel_queued_upload(&mut jobs[5].job).unwrap();
        assert_eq!(jobs[5].job.status, YouTubeJobStatus::Cancelled);
        assert_eq!(
            cancel_queued_upload(&mut jobs[5].job).unwrap_err().code,
            "UPLOAD_NOT_RUNNING"
        );
        jobs[0].job.status = YouTubeJobStatus::Failed;
        running.remove("0");
        assert_eq!(reserve_uploads(&jobs, &mut running)[0].0, "6");
    }

    #[test]
    fn retry_during_worker_teardown_cannot_spawn_a_duplicate_worker() {
        let jobs = vec![queued("retry")];
        let mut running = HashMap::new();
        reserve_uploads(&jobs, &mut running);
        // UI has already put a failed job back in Queued, but its old worker
        // has not yet returned from the final status callback.
        assert!(reserve_uploads(&jobs, &mut running).is_empty());
        running.remove("retry");
        assert_eq!(reserve_uploads(&jobs, &mut running).len(), 1);
    }

    #[test]
    fn restart_restores_all_active_jobs_without_discarding_resume_offsets() {
        let statuses = [
            YouTubeJobStatus::Uploading,
            YouTubeJobStatus::WaitingToRetry,
            YouTubeJobStatus::Queued,
            YouTubeJobStatus::PreparingAuthorization,
            YouTubeJobStatus::Completed,
            YouTubeJobStatus::Cancelled,
            YouTubeJobStatus::Failed,
        ];
        let mut jobs: Vec<_> = statuses
            .iter()
            .enumerate()
            .map(|(i, status)| {
                let mut item = queued(&i.to_string());
                item.job.status = *status;
                item.job.uploaded_bytes = 8;
                item
            })
            .collect();
        assert!(restore_upload_queue(&mut jobs));
        assert!(jobs[..4]
            .iter()
            .all(|item| item.job.status == YouTubeJobStatus::Queued
                && item.job.uploaded_bytes == 8
                && item.job.error_code.is_none()));
        assert_eq!(
            jobs[4..]
                .iter()
                .map(|item| item.job.status)
                .collect::<Vec<_>>(),
            statuses[4..]
        );
        assert_eq!(reserve_uploads(&jobs, &mut HashMap::new()).len(), 4);
    }

    #[test]
    fn simultaneous_dispatch_calls_share_one_capacity_limit() {
        let jobs = Arc::new((0..20).map(|i| queued(&i.to_string())).collect::<Vec<_>>());
        let running = Arc::new(Mutex::new(HashMap::new()));
        let handles: Vec<_> = (0..12)
            .map(|_| {
                let jobs = jobs.clone();
                let running = running.clone();
                std::thread::spawn(move || {
                    reserve_uploads(&jobs, &mut running.lock().unwrap()).len()
                })
            })
            .collect();
        assert_eq!(
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .sum::<usize>(),
            5
        );
        assert_eq!(running.lock().unwrap().len(), 5);
    }

    #[test]
    fn pausing_releases_capacity_after_worker_exit_and_restart_keeps_it_paused() {
        let mut jobs: Vec<_> = (0..7).map(|i| queued(&i.to_string())).collect();
        let mut running = HashMap::new();
        reserve_uploads(&jobs, &mut running);
        pause_upload_job(&mut jobs[0].job, true).unwrap();
        assert_eq!(jobs[0].job.status, YouTubeJobStatus::Pausing);
        assert!(reserve_uploads(&jobs, &mut running).is_empty());
        jobs[0].job.status = YouTubeJobStatus::Paused;
        running.remove("0");
        assert_eq!(reserve_uploads(&jobs, &mut running)[0].0, "5");
        pause_upload_job(&mut jobs[6].job, false).unwrap();
        assert_eq!(jobs[6].job.status, YouTubeJobStatus::Paused);
        jobs[1].job.status = YouTubeJobStatus::Pausing;
        restore_upload_queue(&mut jobs);
        assert_eq!(jobs[0].job.status, YouTubeJobStatus::Paused);
        assert_eq!(jobs[1].job.status, YouTubeJobStatus::Paused);
        assert_eq!(jobs[6].job.status, YouTubeJobStatus::Paused);
        cancel_queued_upload(&mut jobs[6].job).unwrap();
        assert_eq!(jobs[6].job.status, YouTubeJobStatus::Cancelled);
        jobs[0].job.status = YouTubeJobStatus::Queued;
        running.remove("1");
        assert_eq!(reserve_uploads(&jobs, &mut running)[0].0, "0");
        jobs[0].job.status = YouTubeJobStatus::Completed;
        assert!(pause_upload_job(&mut jobs[0].job, false).is_err());
    }

    #[test]
    fn new_uploads_and_retries_reject_only_the_same_active_file_and_channel() {
        let jobs = vec![queued("first")];
        let mut next = queued("second").job;
        assert!(ensure_upload_not_duplicate(&jobs, &next).is_ok());
        next.source_path = jobs[0].job.source_path.clone();
        assert_eq!(
            ensure_upload_not_duplicate(&jobs, &next).unwrap_err().code,
            "UPLOAD_DUPLICATE_ACTIVE"
        );
        next.channel_id = "another-channel".into();
        assert!(ensure_upload_not_duplicate(&jobs, &next).is_ok());
        assert!(ensure_upload_not_duplicate(&jobs, &jobs[0].job).is_ok());
    }

    #[test]
    fn deleting_an_upload_removes_only_the_requested_stable_job_id() {
        let jobs = vec![queued("first"), queued("second")];

        let (remaining, removed_id) = uploads_without_job(&jobs, "first").unwrap();

        assert_eq!(removed_id, "first");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].job.id, "second");
        assert_eq!(
            uploads_without_job(&remaining, "missing").unwrap_err().code,
            "UPLOAD_JOB_NOT_FOUND"
        );
    }

    #[test]
    fn concurrent_thumbnails_keep_independent_files_and_cleanup() {
        let root = std::env::temp_dir().join(format!(
            "hongguo-thumbnail-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = ThumbnailWorkspace::create(&root).unwrap();
        let second = ThumbnailWorkspace::create(&root).unwrap();
        assert_ne!(first.0, second.0);
        fs::write(first.0.join("youtube-thumbnail.jpg"), b"first").unwrap();
        fs::write(second.0.join("youtube-thumbnail.jpg"), b"second").unwrap();
        let first_path = first.0.clone();
        drop(first);
        assert!(!first_path.exists());
        assert_eq!(
            fs::read(second.0.join("youtube-thumbnail.jpg")).unwrap(),
            b"second"
        );
        drop(second);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn local_duplicate_guard_catches_concurrent_and_recently_completed_uploads() {
        use super::super::duplicates::UploadIdentity;
        let mut existing = queued("first");
        existing.intent.dedup = Some(UploadIdentity {
            channel_id: existing.job.channel_id.clone(),
            book_id: "book-1".into(),
            drama_title: "同一部剧".into(),
            season: None,
            allow_duplicate: false,
        });
        let mut incoming = queued("second");
        existing.intent.upload_format = super::super::format::UploadFormat::Standard;
        incoming.intent.upload_format = super::super::format::UploadFormat::Standard;
        incoming.intent.dedup = existing.intent.dedup.clone();
        incoming.intent.title = "改过的标题".into();
        assert_eq!(
            ensure_no_local_duplicate(&[existing.clone()], &incoming.intent, &incoming.job)
                .unwrap_err()
                .code,
            "UPLOAD_DUPLICATE_QUEUED"
        );
        existing.job.status = YouTubeJobStatus::Completed;
        existing.job.video_id = Some("just-uploaded".into());
        assert!(
            ensure_no_local_duplicate(&[existing.clone()], &incoming.intent, &incoming.job)
                .is_err()
        );
        incoming.job.channel_id = "other-channel".into();
        assert!(
            ensure_no_local_duplicate(&[existing.clone()], &incoming.intent, &incoming.job).is_ok()
        );
        incoming.job.channel_id = existing.job.channel_id.clone();
        incoming.intent.dedup.as_mut().unwrap().allow_duplicate = true;
        assert!(
            ensure_no_local_duplicate(&[existing.clone()], &incoming.intent, &incoming.job).is_ok()
        );
        // An override must still not enqueue the same active source file twice.
        existing.job.status = YouTubeJobStatus::Uploading;
        incoming.job.source_path = existing.job.source_path.clone();
        assert_eq!(
            ensure_upload_not_duplicate(&[existing], &incoming.job)
                .unwrap_err()
                .code,
            "UPLOAD_DUPLICATE_ACTIVE"
        );
    }
    #[test]
    fn local_guard_and_history_preserve_season_and_actual_format() {
        use super::super::{duplicates::UploadIdentity, format::UploadFormat};
        let mut existing = queued("first");
        existing.intent.upload_format = UploadFormat::Standard;
        existing.intent.dedup = Some(UploadIdentity {
            channel_id: existing.job.channel_id.clone(),
            book_id: "book-1".into(),
            drama_title: "作品第二季".into(),
            season: Some(2),
            allow_duplicate: false,
        });
        let mut incoming = queued("second");
        incoming.intent = existing.intent.clone();
        incoming.intent.dedup.as_mut().unwrap().season = Some(3);
        assert!(
            ensure_no_local_duplicate(&[existing.clone()], &incoming.intent, &incoming.job).is_ok()
        );
        incoming.intent.dedup.as_mut().unwrap().season = Some(2);
        incoming.intent.upload_format = UploadFormat::Shorts;
        assert!(
            ensure_no_local_duplicate(&[existing.clone()], &incoming.intent, &incoming.job).is_ok()
        );
        incoming.intent.upload_format = UploadFormat::Standard;
        existing.intent.upload_format = UploadFormat::Auto;
        assert_eq!(
            ensure_no_local_duplicate(&[existing.clone()], &incoming.intent, &incoming.job)
                .unwrap_err()
                .code,
            "UPLOAD_DUPLICATE_REVIEW_REQUIRED"
        );
        existing.intent.upload_format = UploadFormat::Shorts;
        let history = known_video(&existing, "uploaded");
        assert_eq!(history.season, Some(2));
        assert_eq!(history.drama_title, "作品第二季");
        assert_eq!(history.upload_format, UploadFormat::Shorts);
    }

    #[test]
    fn restart_never_requeues_a_video_that_already_has_a_youtube_id() {
        for has_cover in [false, true] {
            let mut stored = queued("finished-video");
            stored.job.status = YouTubeJobStatus::Processing;
            stored.job.video_id = Some("uploaded-video".into());
            stored.job.youtube_url = Some("https://www.youtube.com/watch?v=uploaded-video".into());
            stored.intent.cover_path = has_cover.then(|| PathBuf::from("/cover.png"));
            let mut uploads = vec![stored];
            assert!(restore_upload_queue(&mut uploads));
            assert_eq!(
                uploads[0].job.status,
                if has_cover {
                    YouTubeJobStatus::VideoUploadedThumbnailFailed
                } else {
                    YouTubeJobStatus::Completed
                }
            );
            assert_eq!(uploads[0].job.video_id.as_deref(), Some("uploaded-video"));
            assert_eq!(uploads[0].job.percent, 100.0);
            assert!(reserve_uploads(&uploads, &mut HashMap::new()).is_empty());
        }
    }
}
