use super::{
    config::{import_private, OAuthClientConfig},
    models::{
        AccountSummary, CredentialSummary, ThumbnailState, UploadIntent, YouTubeJob,
        YouTubeJobStatus, YouTubeSnapshot,
    },
    oauth::OAuthService,
    state::YouTubeStateStore,
    thumbnail::{prepare_thumbnail, set_thumbnail},
    upload::{RefreshCallback, ResumableUploader, UploadCancellationToken, UploadProgressEvent},
    vault::{OsTokenVault, TokenVault},
};
use crate::{
    media::{MediaJobService, MediaTools},
    AppError,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
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

    pub fn start_upload(self: &Arc<Self>, intent: UploadIntent) -> Result<YouTubeJob, AppError> {
        intent.validate()?;
        if !self
            .media_jobs
            .is_validated_upload_source(&intent.file_path)
        {
            return Err(AppError::new(
                "UPLOAD_SOURCE_NOT_MERGED",
                "只能上传由应用验证完成的合并视频",
            ));
        }
        let channel_id = self
            .state
            .snapshot()
            .active_channel_id
            .ok_or_else(|| AppError::new("AUTH_REQUIRED", "请先授权并选择 YouTube 频道"))?;
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

    pub async fn retry_thumbnail(&self, job_id: &str) -> Result<YouTubeJob, AppError> {
        let stored = self
            .uploads
            .lock()
            .map_err(state_lock_error)?
            .iter()
            .find(|item| item.job.id == job_id)
            .cloned()
            .ok_or_else(|| AppError::new("UPLOAD_JOB_NOT_FOUND", "YouTube 上传任务不存在"))?;
        if stored.job.status != YouTubeJobStatus::VideoUploadedThumbnailFailed {
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
        match self.publish_thumbnail(&stored, &oauth, &video_id).await {
            Ok(state) => self.update_job(job_id, |job| {
                job.thumbnail_state = state;
                job.status = YouTubeJobStatus::Completed;
                job.error_code = None;
                job.error_message = None;
                Ok(())
            }),
            Err(error) => self.update_job(job_id, |job| {
                job.thumbnail_state = ThumbnailState::Failed;
                job.status = YouTubeJobStatus::VideoUploadedThumbnailFailed;
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
                    YouTubeJobStatus::Failed | YouTubeJobStatus::VideoUploadedThumbnailFailed
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
                        }
                        Err(ref error) => {
                            job.thumbnail_state = ThumbnailState::Failed;
                            job.status = YouTubeJobStatus::VideoUploadedThumbnailFailed;
                            job.percent = 100.0;
                            job.error_code = Some(error.code.clone());
                            job.error_message = Some(error.message.clone());
                        }
                    }
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

fn restore_upload_queue(uploads: &mut [StoredUpload]) -> bool {
    let mut changed = false;
    for item in uploads {
        if item.job.status == YouTubeJobStatus::Pausing {
            item.job.status = YouTubeJobStatus::Paused;
            changed = true;
        } else if is_active(item.job.status) {
            item.job.status = YouTubeJobStatus::Queued;
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
    fs::rename(temporary, target).map_err(service_io)?;
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(service_io)
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
                completion_notified_at: None,
                failure_notified_at: None,
            },
            intent: UploadIntent {
                job_id: id.into(),
                file_path: PathBuf::from(format!("/{id}.mp4")),
                cover_path: None,
                title: id.into(),
                description: String::new(),
                tags: vec![],
                category_id: "24".into(),
                privacy_status: PrivacyStatus::Private,
                self_declared_made_for_kids: false,
                contains_synthetic_media: false,
                audience_confirmed: true,
                synthetic_media_confirmed: true,
                publish_confirmed: true,
            },
        }
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
}
