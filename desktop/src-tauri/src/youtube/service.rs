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
        let mut resume_job_id = None;
        for item in &mut uploads {
            if is_active(item.job.status) && resume_job_id.is_none() {
                item.job.status = YouTubeJobStatus::Queued;
                resume_job_id = Some(item.job.id.clone());
            } else if is_active(item.job.status) {
                item.job.status = YouTubeJobStatus::Failed;
                item.job.error_code = Some("UPLOAD_INTERRUPTED".into());
                item.job.error_message = Some("应用退出时上传被中断，可手动重试".into());
            }
        }
        if resume_job_id.is_some() {
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
        if let Some(job_id) = resume_job_id {
            service.spawn_upload(job_id)?;
        }
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

    pub async fn authorize(&self) -> Result<AccountSummary, AppError> {
        let service = self.oauth_service()?;
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
            if uploads.iter().any(|item| is_active(item.job.status)) {
                return Err(AppError::new(
                    "UPLOAD_ALREADY_RUNNING",
                    "当前已有 YouTube 上传任务",
                ));
            }
            uploads.push(StoredUpload {
                job: job.clone(),
                intent,
            });
            persist_uploads(&self.data_dir, &uploads)?;
        }
        self.event_sink.emit(job.clone());
        self.spawn_upload(job.id.clone())?;
        Ok(job)
    }

    pub fn retry_upload(self: &Arc<Self>, job_id: &str) -> Result<YouTubeJob, AppError> {
        let job = self.update_job(job_id, |job| {
            if !matches!(
                job.status,
                YouTubeJobStatus::Failed | YouTubeJobStatus::Cancelled
            ) {
                return Err(AppError::new(
                    "UPLOAD_INVALID_TRANSITION",
                    "当前 YouTube 任务不能重试",
                ));
            }
            job.status = YouTubeJobStatus::Queued;
            job.error_code = None;
            job.error_message = None;
            job.completion_notified_at = None;
            job.failure_notified_at = None;
            Ok(())
        })?;
        self.spawn_upload(job_id.to_string())?;
        Ok(job)
    }

    pub fn cancel_upload(&self, job_id: &str) -> Result<(), AppError> {
        self.running
            .lock()
            .map_err(state_lock_error)?
            .get(job_id)
            .cloned()
            .ok_or_else(|| AppError::new("UPLOAD_NOT_RUNNING", "YouTube 上传任务未运行"))?
            .cancel();
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

    fn spawn_upload(self: &Arc<Self>, job_id: String) -> Result<(), AppError> {
        let cancellation = UploadCancellationToken::default();
        self.running
            .lock()
            .map_err(state_lock_error)?
            .insert(job_id.clone(), cancellation.clone());
        let service = self.clone();
        tauri::async_runtime::spawn(async move {
            service
                .clone()
                .run_upload(job_id.clone(), cancellation)
                .await;
            if let Ok(mut running) = service.running.lock() {
                running.remove(&job_id);
            }
        });
        Ok(())
    }

    async fn run_upload(self: Arc<Self>, job_id: String, cancellation: UploadCancellationToken) {
        let stored = self
            .uploads
            .lock()
            .ok()
            .and_then(|items| items.iter().find(|item| item.job.id == job_id).cloned());
        let Some(stored) = stored else { return };
        let oauth = match self.oauth_service() {
            Ok(value) => Arc::new(value),
            Err(error) => {
                let _ = self.fail_job(&job_id, error);
                return;
            }
        };
        let token = match oauth.access_token(&stored.job.channel_id).await {
            Ok(value) => value,
            Err(error) => {
                let _ = self.fail_job(&job_id, error);
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
                job.status = event.status;
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
            Err(error) if error.code == "UPLOAD_CANCELLED" => {
                let _ = self.update_job(&job_id, |job| {
                    job.status = YouTubeJobStatus::Cancelled;
                    Ok(())
                });
            }
            Err(error) => {
                let _ = self.fail_job(&job_id, error);
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
        let temp = self.data_dir.join("youtube/thumbnail-temp");
        fs::create_dir_all(&temp).map_err(service_io)?;
        let executable = std::env::current_exe().map_err(service_io)?;
        let tools = executable
            .parent()
            .ok_or_else(|| AppError::new("MEDIA_TOOL_MISSING", "无法定位媒体工具"))
            .and_then(MediaTools::from_resource_root)?;
        let prepared = prepare_thumbnail(cover, &tools, &temp)?;
        let token = oauth.access_token(&stored.job.channel_id).await?;
        set_thumbnail(&reqwest::Client::new(), video_id, &prepared, &token).await?;
        let _ = fs::remove_file(prepared);
        Ok(ThumbnailState::Succeeded)
    }

    fn fail_job(&self, job_id: &str, error: AppError) -> Result<YouTubeJob, AppError> {
        self.update_job(job_id, |job| {
            job.status = YouTubeJobStatus::Failed;
            job.error_code = Some(error.code);
            job.error_message = Some(error.message);
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

fn is_active(status: YouTubeJobStatus) -> bool {
    matches!(
        status,
        YouTubeJobStatus::Queued
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
