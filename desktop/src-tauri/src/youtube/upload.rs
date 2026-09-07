use super::{
    config::SecretString,
    models::{PrivacyStatus, ThumbnailState, UploadIntent, UploadResult, YouTubeJobStatus},
};
use crate::AppError;
use reqwest::{
    header::{CONTENT_LENGTH, CONTENT_RANGE, LOCATION, RANGE},
    Client, Response, StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, File, OpenOptions},
    future::Future,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
    time::{Duration, UNIX_EPOCH},
};
use tokio::{sync::Notify, time::sleep};
use url::Url;

const UPLOAD_ENDPOINT: &str = "https://www.googleapis.com/upload/youtube/v3/videos";
pub const CHUNK_SIZE: u64 = 8 * 1024 * 1024;

#[derive(Default)]
struct UploadStopState {
    // 0 = running, 1 = pause requested, 2 = cancel requested.
    mode: AtomicU8,
    wake: Notify,
}

#[derive(Clone, Default)]
pub struct UploadCancellationToken(Arc<UploadStopState>);

impl UploadCancellationToken {
    pub fn cancel(&self) {
        self.0.mode.store(2, Ordering::Release);
        self.0.wake.notify_one();
    }

    pub fn pause(&self) {
        let _ = self
            .0
            .mode
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
        self.0.wake.notify_one();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.mode.load(Ordering::Acquire) == 2
    }
    pub fn is_paused(&self) -> bool {
        self.0.mode.load(Ordering::Acquire) == 1
    }

    async fn stopped(&self) {
        while self.0.mode.load(Ordering::Acquire) == 0 {
            self.0.wake.notified().await;
        }
    }

    pub async fn interruptible<T>(&self, work: impl Future<Output = T>) -> Result<T, AppError> {
        tokio::select! {
            biased;
            _ = self.stopped() => {
                ensure_not_cancelled(self)?;
                unreachable!("stop signal always has a reason")
            }
            result = work => Ok(result),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UploadProgressEvent {
    pub status: YouTubeJobStatus,
    pub uploaded_bytes: u64,
    pub total_bytes: u64,
    pub percent: f64,
}

pub type RefreshFuture = Pin<Box<dyn Future<Output = Result<SecretString, AppError>> + Send>>;
pub type RefreshCallback = Arc<dyn Fn() -> RefreshFuture + Send + Sync>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadCheckpoint {
    pub file_path: PathBuf,
    pub file_size: u64,
    pub modified_unix_nanos: u128,
    pub uploaded_offset: u64,
    session_url: String,
    pub metadata_hash: String,
    pub channel_hash: String,
}

impl std::fmt::Debug for UploadCheckpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UploadCheckpoint")
            .field("file_path", &"[REDACTED]")
            .field("file_size", &self.file_size)
            .field("modified_unix_nanos", &self.modified_unix_nanos)
            .field("uploaded_offset", &self.uploaded_offset)
            .field("session_url", &"[REDACTED]")
            .field("metadata_hash", &self.metadata_hash)
            .field("channel_hash", &self.channel_hash)
            .finish()
    }
}

enum SessionStatus {
    Incomplete(u64),
    Complete(UploadResult),
}

fn retry_delay(attempt: u32) -> Duration {
    Duration::from_secs(1 << (attempt - 1).min(4))
}
fn network_failed() -> AppError {
    AppError::new(
        "UPLOAD_NETWORK_FAILED",
        "上传连接失败，请检查网络或代理后重试，已上传进度会保留",
    )
}
fn session_expired() -> AppError {
    AppError::new(
        "UPLOAD_SESSION_EXPIRED",
        "YouTube 上传会话已过期，需要重新创建上传任务",
    )
}
async fn response_result(response: Response) -> Result<UploadResult, AppError> {
    let bytes = response.bytes().await.map_err(|_| {
        AppError::new(
            "UPLOAD_RESPONSE_INVALID",
            "YouTube 上传响应无效，可重试查询结果",
        )
    })?;
    parse_upload_result(&bytes)
}
fn finish_upload(
    result: UploadResult,
    checkpoint: &Path,
    progress: &Arc<dyn Fn(UploadProgressEvent) + Send + Sync>,
    total: u64,
) -> Result<UploadResult, AppError> {
    emit(progress, YouTubeJobStatus::Processing, total, total);
    let _ = fs::remove_file(checkpoint);
    Ok(result)
}

struct SourceSnapshot {
    path: PathBuf,
    size: u64,
    modified_unix_nanos: u128,
}

pub struct ResumableUploader {
    client: Client,
    endpoint: Url,
    checkpoint_dir: PathBuf,
}

impl ResumableUploader {
    pub fn new(checkpoint_dir: PathBuf) -> Self {
        Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(180))
                .build()
                .expect("fixed upload client configuration is valid"),
            endpoint: Url::parse(UPLOAD_ENDPOINT).expect("fixed upload endpoint is valid"),
            checkpoint_dir,
        }
    }

    pub async fn upload(
        &self,
        intent: &UploadIntent,
        channel_id: &str,
        initial_token: SecretString,
        refresh: RefreshCallback,
        cancellation: UploadCancellationToken,
        progress: Arc<dyn Fn(UploadProgressEvent) + Send + Sync>,
    ) -> Result<UploadResult, AppError> {
        ensure_not_cancelled(&cancellation)?;
        intent.validate()?;
        let source = snapshot_source(&intent.file_path)?;
        fs::create_dir_all(&self.checkpoint_dir).map_err(upload_io)?;
        let checkpoint_path = self.checkpoint_dir.join(format!("{}.json", intent.job_id));
        let metadata_hash = intent_hash(intent);
        let channel_hash = hash_text(channel_id);
        let mut token = initial_token;
        let mut refreshed = false;
        let saved = load_checkpoint(&checkpoint_path)?;
        if let Some(value) = &saved {
            validate_checkpoint(value, &source, &metadata_hash, &channel_hash)?;
        }
        emit(
            &progress,
            YouTubeJobStatus::PreparingAuthorization,
            saved.as_ref().map_or(0, |value| value.uploaded_offset),
            source.size,
        );
        let mut needs_query = saved.is_some();
        let mut checkpoint = match saved {
            Some(value) => value,
            None => {
                emit(&progress, YouTubeJobStatus::CreatingSession, 0, source.size);
                // Finish saving a newly created session before honouring pause;
                // dropping this POST can lose the session URL.
                let session = self
                    .create_session(intent, &source, &mut token, &refresh, &mut refreshed)
                    .await?;
                let value = UploadCheckpoint {
                    file_path: source.path.clone(),
                    file_size: source.size,
                    modified_unix_nanos: source.modified_unix_nanos,
                    uploaded_offset: 0,
                    session_url: session.to_string(),
                    metadata_hash,
                    channel_hash,
                };
                save_checkpoint(&checkpoint_path, &value)?;
                value
            }
        };
        let session = validate_session_url(&checkpoint.session_url)?;
        let mut file = File::open(&source.path).map_err(upload_io)?;
        let mut failures = 0;
        loop {
            ensure_not_cancelled(&cancellation)?;
            if needs_query {
                match self
                    .recover_session(
                        &session,
                        source.size,
                        &mut token,
                        &refresh,
                        &mut refreshed,
                        &cancellation,
                        &progress,
                        checkpoint.uploaded_offset,
                    )
                    .await?
                {
                    SessionStatus::Complete(result) => {
                        return finish_upload(result, &checkpoint_path, &progress, source.size)
                    }
                    SessionStatus::Incomplete(offset) => {
                        checkpoint.uploaded_offset = offset;
                        save_checkpoint(&checkpoint_path, &checkpoint)?;
                        emit(&progress, YouTubeJobStatus::Uploading, offset, source.size);
                    }
                }
                needs_query = false;
            }
            ensure_not_cancelled(&cancellation)?;
            if checkpoint.uploaded_offset >= source.size {
                return Err(AppError::new(
                    "UPLOAD_CHECKPOINT_INVALID",
                    "上传检查点字节位置无效",
                ));
            }
            let start = checkpoint.uploaded_offset;
            let length = (source.size - start).min(CHUNK_SIZE);
            let end = start + length - 1;
            file.seek(SeekFrom::Start(start)).map_err(upload_io)?;
            let mut bytes = vec![0u8; length as usize];
            file.read_exact(&mut bytes).map_err(upload_io)?;
            let mut response = cancellation
                .interruptible(self.send_chunk(
                    &session,
                    start,
                    end,
                    source.size,
                    bytes.clone(),
                    &token,
                ))
                .await?;
            if response
                .as_ref()
                .is_ok_and(|value| value.status() == StatusCode::UNAUTHORIZED)
                && !refreshed
            {
                token = cancellation.interruptible(refresh()).await??;
                refreshed = true;
                response = cancellation
                    .interruptible(self.send_chunk(
                        &session,
                        start,
                        end,
                        source.size,
                        bytes,
                        &token,
                    ))
                    .await?;
            }
            match response {
                Ok(value) if value.status().as_u16() == 308 => {
                    let offset = next_offset(&value)?;
                    if offset >= source.size || offset <= start || offset > end + 1 {
                        return Err(AppError::new(
                            "UPLOAD_SERVER_OFFSET_INVALID",
                            "YouTube 返回的上传位置无效",
                        ));
                    }
                    checkpoint.uploaded_offset = offset;
                    save_checkpoint(&checkpoint_path, &checkpoint)?;
                    emit(&progress, YouTubeJobStatus::Uploading, offset, source.size);
                    failures = 0;
                }
                Ok(value) if value.status().is_success() => {
                    let result = cancellation.interruptible(response_result(value)).await??;
                    return finish_upload(result, &checkpoint_path, &progress, source.size);
                }
                Ok(value) if matches!(value.status(), StatusCode::NOT_FOUND | StatusCode::GONE) => {
                    return Err(session_expired());
                }
                Ok(value) if value.status() == StatusCode::UNAUTHORIZED => {
                    return Err(AppError::new("AUTH_REQUIRED", "需要重新授权 YouTube 频道"));
                }
                Ok(value)
                    if !value.status().is_server_error()
                        && value.status() != StatusCode::TOO_MANY_REQUESTS =>
                {
                    return Err(AppError::new(
                        "UPLOAD_REQUEST_FAILED",
                        "YouTube 上传请求失败",
                    ));
                }
                _ => {
                    failures += 1;
                    if failures > 5 {
                        return Err(network_failed());
                    }
                    emit(
                        &progress,
                        YouTubeJobStatus::WaitingToRetry,
                        checkpoint.uploaded_offset,
                        source.size,
                    );
                    cancellation
                        .interruptible(sleep(retry_delay(failures)))
                        .await?;
                    // A broken connection may have delivered all or part of the
                    // chunk. Query server truth before sending any more bytes.
                    needs_query = true;
                }
            }
        }
    }

    async fn create_session(
        &self,
        intent: &UploadIntent,
        source: &SourceSnapshot,
        token: &mut SecretString,
        refresh: &RefreshCallback,
        refreshed: &mut bool,
    ) -> Result<Url, AppError> {
        let body = json!({
            "snippet": {
                "title": intent.title,
                "description": intent.description,
                "tags": intent.tags,
                "categoryId": intent.category_id,
            },
            "status": {
                "privacyStatus": intent.privacy_status,
                "selfDeclaredMadeForKids": intent.self_declared_made_for_kids,
                "containsSyntheticMedia": intent.contains_synthetic_media,
            }
        });
        let mut response = self
            .client
            .post(self.endpoint.clone())
            .timeout(Duration::from_secs(30))
            .query(&[("uploadType", "resumable"), ("part", "snippet,status")])
            .bearer_auth(token.expose_secret())
            .header("X-Upload-Content-Type", "video/*")
            .header("X-Upload-Content-Length", source.size)
            .json(&body)
            .send()
            .await
            .map_err(|_| AppError::new("UPLOAD_SESSION_FAILED", "无法创建 YouTube 上传会话"))?;
        if response.status() == StatusCode::UNAUTHORIZED && !*refreshed {
            *token = refresh().await?;
            *refreshed = true;
            response = self
                .client
                .post(self.endpoint.clone())
                .timeout(Duration::from_secs(30))
                .query(&[("uploadType", "resumable"), ("part", "snippet,status")])
                .bearer_auth(token.expose_secret())
                .header("X-Upload-Content-Type", "video/*")
                .header("X-Upload-Content-Length", source.size)
                .json(&body)
                .send()
                .await
                .map_err(|_| AppError::new("UPLOAD_SESSION_FAILED", "无法创建 YouTube 上传会话"))?;
        }
        if !response.status().is_success() {
            return Err(AppError::new(
                "UPLOAD_SESSION_FAILED",
                "无法创建 YouTube 上传会话",
            ));
        }
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| AppError::new("UPLOAD_SESSION_FAILED", "YouTube 上传会话响应无效"))?;
        validate_session_url(location)
    }

    async fn send_chunk(
        &self,
        session: &Url,
        start: u64,
        end: u64,
        total: u64,
        bytes: Vec<u8>,
        token: &SecretString,
    ) -> Result<Response, reqwest::Error> {
        self.client
            .put(session.clone())
            .bearer_auth(token.expose_secret())
            .header(CONTENT_LENGTH, bytes.len())
            .header(CONTENT_RANGE, format!("bytes {start}-{end}/{total}"))
            .body(bytes)
            .send()
            .await
    }

    async fn query_status(
        &self,
        session: &Url,
        total: u64,
        token: &SecretString,
    ) -> Result<SessionStatus, AppError> {
        let response = self
            .client
            .put(session.clone())
            .bearer_auth(token.expose_secret())
            .header(CONTENT_LENGTH, 0)
            .header(CONTENT_RANGE, format!("bytes */{total}"))
            .send()
            .await
            .map_err(|_| network_failed())?;
        match response.status() {
            StatusCode::NOT_FOUND | StatusCode::GONE => Err(session_expired()),
            StatusCode::UNAUTHORIZED => {
                Err(AppError::new("AUTH_REQUIRED", "需要重新授权 YouTube 频道"))
            }
            status if status.as_u16() == 308 => {
                let offset = next_offset(&response)?;
                if offset >= total {
                    return Err(AppError::new(
                        "UPLOAD_SERVER_OFFSET_INVALID",
                        "YouTube 返回的上传位置无效",
                    ));
                }
                Ok(SessionStatus::Incomplete(offset))
            }
            status if status.is_success() => {
                Ok(SessionStatus::Complete(response_result(response).await?))
            }
            status if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS => {
                Err(network_failed())
            }
            _ => Err(AppError::new(
                "UPLOAD_REQUEST_FAILED",
                "无法恢复 YouTube 上传，请检查账号权限",
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn recover_session(
        &self,
        session: &Url,
        total: u64,
        token: &mut SecretString,
        refresh: &RefreshCallback,
        refreshed: &mut bool,
        cancellation: &UploadCancellationToken,
        progress: &Arc<dyn Fn(UploadProgressEvent) + Send + Sync>,
        offset: u64,
    ) -> Result<SessionStatus, AppError> {
        let mut failures = 0;
        loop {
            match cancellation
                .interruptible(self.query_status(session, total, token))
                .await?
            {
                Err(error) if error.code == "AUTH_REQUIRED" && !*refreshed => {
                    *token = cancellation.interruptible(refresh()).await??;
                    *refreshed = true;
                }
                Err(error) if error.code == "UPLOAD_NETWORK_FAILED" && failures < 5 => {
                    failures += 1;
                    emit(progress, YouTubeJobStatus::WaitingToRetry, offset, total);
                    cancellation
                        .interruptible(sleep(retry_delay(failures)))
                        .await?;
                }
                result => return result,
            }
        }
    }
}

fn snapshot_source(path: &Path) -> Result<SourceSnapshot, AppError> {
    let path = fs::canonicalize(path).map_err(upload_io)?;
    let metadata = fs::metadata(&path).map_err(upload_io)?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(AppError::new("UPLOAD_SOURCE_INVALID", "上传源文件无效"));
    }
    let modified_unix_nanos = metadata
        .modified()
        .map_err(upload_io)?
        .duration_since(UNIX_EPOCH)
        .map_err(upload_io)?
        .as_nanos();
    Ok(SourceSnapshot {
        path,
        size: metadata.len(),
        modified_unix_nanos,
    })
}

fn validate_checkpoint(
    checkpoint: &UploadCheckpoint,
    source: &SourceSnapshot,
    metadata_hash: &str,
    channel_hash: &str,
) -> Result<(), AppError> {
    if checkpoint.file_path != source.path
        || checkpoint.file_size != source.size
        || checkpoint.modified_unix_nanos != source.modified_unix_nanos
    {
        return Err(AppError::new(
            "UPLOAD_SOURCE_CHANGED",
            "上传源文件已发生变化",
        ));
    }
    if checkpoint.metadata_hash != metadata_hash || checkpoint.channel_hash != channel_hash {
        return Err(AppError::new(
            "UPLOAD_INTENT_CHANGED",
            "上传元数据或频道已发生变化",
        ));
    }
    if checkpoint.uploaded_offset >= source.size {
        return Err(AppError::new("UPLOAD_CHECKPOINT_INVALID", "上传检查点无效"));
    }
    validate_session_url(&checkpoint.session_url)?;
    Ok(())
}

fn validate_session_url(value: &str) -> Result<Url, AppError> {
    let url = Url::parse(value)
        .map_err(|_| AppError::new("UPLOAD_SESSION_INVALID", "YouTube 上传会话无效"))?;
    let production = url.scheme() == "https"
        && matches!(
            url.host_str(),
            Some("www.googleapis.com" | "upload.youtube.com")
        );
    #[cfg(test)]
    let allowed = production
        || (url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost")));
    #[cfg(not(test))]
    let allowed = production;
    if !allowed
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(AppError::new(
            "UPLOAD_SESSION_INVALID",
            "YouTube 上传会话无效",
        ));
    }
    Ok(url)
}

fn next_offset(response: &Response) -> Result<u64, AppError> {
    match response
        .headers()
        .get(RANGE)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) => value
            .strip_prefix("bytes=0-")
            .and_then(|value| value.parse::<u64>().ok())
            .and_then(|last| last.checked_add(1))
            .ok_or_else(|| {
                AppError::new("UPLOAD_SERVER_OFFSET_INVALID", "YouTube 返回的上传位置无效")
            }),
        None => Ok(0),
    }
}

fn parse_upload_result(bytes: &[u8]) -> Result<UploadResult, AppError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| AppError::new("UPLOAD_RESPONSE_INVALID", "YouTube 上传响应无效"))?;
    let video_id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 64)
        .ok_or_else(|| AppError::new("UPLOAD_RESPONSE_INVALID", "YouTube 上传响应缺少视频 ID"))?
        .to_string();
    let privacy_status = match value
        .pointer("/status/privacyStatus")
        .and_then(Value::as_str)
    {
        Some("private") => Some(PrivacyStatus::Private),
        Some("unlisted") => Some(PrivacyStatus::Unlisted),
        Some("public") => Some(PrivacyStatus::Public),
        _ => None,
    };
    Ok(UploadResult {
        youtube_url: format!("https://www.youtube.com/watch?v={video_id}"),
        video_id,
        privacy_status,
        thumbnail_state: ThumbnailState::Pending,
    })
}

fn intent_hash(intent: &UploadIntent) -> String {
    hash_text(&format!(
        "{}\0{}\0{:?}\0{}\0{}\0{}\0{}",
        intent.title,
        intent.description,
        intent.tags,
        intent.category_id,
        intent.privacy_status as u8,
        intent.self_declared_made_for_kids,
        intent.contains_synthetic_media,
    ))
}

fn hash_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn load_checkpoint(path: &Path) -> Result<Option<UploadCheckpoint>, AppError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| AppError::new("UPLOAD_CHECKPOINT_INVALID", "上传检查点已损坏")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(upload_io(error)),
    }
}

fn save_checkpoint(path: &Path, value: &UploadCheckpoint) -> Result<(), AppError> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("UPLOAD_IO", "YouTube 上传文件操作失败"))?;
    protect_checkpoint_path(parent, true)?;
    let temporary = path.with_extension("json.tmp");
    let mut file = checkpoint_write_options()
        .open(&temporary)
        .map_err(upload_io)?;
    protect_checkpoint_path(&temporary, false)?;
    serde_json::to_writer(&mut file, value).map_err(upload_io)?;
    file.flush()
        .and_then(|_| file.sync_all())
        .map_err(upload_io)?;
    drop(file);
    fs::rename(temporary, path).map_err(upload_io)
}

fn checkpoint_write_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    options
}

#[cfg(unix)]
fn protect_checkpoint_path(path: &Path, directory: bool) -> Result<(), AppError> {
    let mode = if directory { 0o700 } else { 0o600 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(upload_io)
}

#[cfg(windows)]
fn protect_checkpoint_path(_path: &Path, _directory: bool) -> Result<(), AppError> {
    Ok(())
}

fn emit(
    callback: &Arc<dyn Fn(UploadProgressEvent) + Send + Sync>,
    status: YouTubeJobStatus,
    uploaded: u64,
    total: u64,
) {
    callback(UploadProgressEvent {
        status,
        uploaded_bytes: uploaded,
        total_bytes: total,
        percent: if total == 0 {
            0.0
        } else {
            uploaded as f64 / total as f64 * 100.0
        },
    });
}

fn ensure_not_cancelled(cancellation: &UploadCancellationToken) -> Result<(), AppError> {
    if cancellation.is_paused() {
        return Err(AppError::new("UPLOAD_PAUSED", "YouTube 上传已暂停"));
    }
    if cancellation.is_cancelled() {
        Err(AppError::new("UPLOAD_CANCELLED", "YouTube 上传已取消"))
    } else {
        Ok(())
    }
}

fn upload_io(error: impl std::fmt::Display) -> AppError {
    AppError::with_cause("UPLOAD_IO", "YouTube 上传文件操作失败", error.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        io::{Read, Write},
        net::TcpListener,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Mutex,
        },
        thread,
        time::Duration,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "hongguo-youtube-upload-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[derive(Clone)]
    struct MockReply {
        status: &'static str,
        headers: Vec<(&'static str, String)>,
        body: &'static str,
    }

    #[derive(Debug, Clone)]
    struct RecordedRequest {
        line: String,
        headers: HashMap<String, String>,
        body_bytes: usize,
    }

    struct MockUploadServer {
        endpoint: Url,
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        shutdown: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl MockUploadServer {
        fn serve(build_replies: impl FnOnce(&str) -> Vec<MockReply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let replies = build_replies(&format!("{base}/session"));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let shutdown = Arc::new(AtomicBool::new(false));
            let thread = thread::spawn({
                let requests = requests.clone();
                let shutdown = shutdown.clone();
                move || {
                    for reply in replies {
                        let mut stream = loop {
                            match listener.accept() {
                                Ok((stream, _)) => break stream,
                                Err(error)
                                    if error.kind() == std::io::ErrorKind::WouldBlock
                                        && !shutdown.load(Ordering::Acquire) =>
                                {
                                    thread::sleep(Duration::from_millis(2));
                                }
                                Err(_) => return,
                            }
                            if shutdown.load(Ordering::Acquire) {
                                return;
                            }
                        };
                        stream.set_nonblocking(false).unwrap();
                        let mut bytes = Vec::new();
                        let header_end = loop {
                            let mut chunk = [0u8; 4096];
                            let count = stream.read(&mut chunk).unwrap();
                            if count == 0 {
                                return;
                            }
                            bytes.extend_from_slice(&chunk[..count]);
                            if let Some(position) =
                                bytes.windows(4).position(|value| value == b"\r\n\r\n")
                            {
                                break position + 4;
                            }
                        };
                        let header_text = String::from_utf8_lossy(&bytes[..header_end]);
                        let mut lines = header_text.split("\r\n");
                        let line = lines.next().unwrap_or_default().to_string();
                        let headers = lines
                            .filter_map(|line| line.split_once(':'))
                            .map(|(name, value)| {
                                (name.trim().to_ascii_lowercase(), value.trim().to_string())
                            })
                            .collect::<HashMap<_, _>>();
                        let content_length = headers
                            .get("content-length")
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(0);
                        let body_already_read = bytes.len().saturating_sub(header_end);
                        let mut remaining = content_length.saturating_sub(body_already_read);
                        while remaining > 0 {
                            let mut chunk = [0u8; 8192];
                            let capacity = remaining.min(chunk.len());
                            let count = stream.read(&mut chunk[..capacity]).unwrap();
                            if count == 0 {
                                return;
                            }
                            remaining -= count;
                        }
                        requests.lock().unwrap().push(RecordedRequest {
                            line,
                            headers,
                            body_bytes: content_length,
                        });
                        let mut response = format!(
                            "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                            reply.status,
                            reply.body.len()
                        );
                        for (name, value) in reply.headers {
                            response.push_str(&format!("{name}: {value}\r\n"));
                        }
                        response.push_str("\r\n");
                        stream.write_all(response.as_bytes()).unwrap();
                        stream.write_all(reply.body.as_bytes()).unwrap();
                        stream.flush().unwrap();
                    }
                }
            });
            Self {
                endpoint: Url::parse(&format!("{base}/videos")).unwrap(),
                requests,
                shutdown,
                thread: Some(thread),
            }
        }

        fn uploader(&self, checkpoint_dir: PathBuf) -> ResumableUploader {
            ResumableUploader {
                client: Client::builder().no_proxy().build().unwrap(),
                endpoint: self.endpoint.clone(),
                checkpoint_dir,
            }
        }

        fn requests(&self) -> Vec<RecordedRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Drop for MockUploadServer {
        fn drop(&mut self) {
            self.shutdown.store(true, Ordering::Release);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn upload_intent(root: &Path, size: usize, job_id: &str) -> UploadIntent {
        let source = root.join("video.mp4");
        fs::write(&source, vec![7u8; size]).unwrap();
        UploadIntent {
            job_id: job_id.into(),
            file_path: source,
            cover_path: None,
            title: "测试短剧".into(),
            description: "本地协议测试".into(),
            tags: vec!["短剧".into()],
            category_id: "24".into(),
            privacy_status: PrivacyStatus::Private,
            self_declared_made_for_kids: false,
            contains_synthetic_media: false,
            audience_confirmed: true,
            synthetic_media_confirmed: true,
            publish_confirmed: true,
        }
    }

    fn no_refresh() -> RefreshCallback {
        Arc::new(|| {
            Box::pin(async {
                Err(AppError::new("UNEXPECTED_REFRESH", "测试中不应刷新令牌"))
            })
        })
    }

    #[tokio::test]
    async fn cancelled_before_start_does_not_create_a_session_or_checkpoint() {
        let root = temp_path("cancel-before-start");
        fs::create_dir_all(&root).unwrap();
        let intent = upload_intent(&root, 16, "cancel-before-start");
        let cancellation = UploadCancellationToken::default();
        cancellation.cancel();
        let checkpoint_dir = root.join("checkpoints");
        let error = ResumableUploader::new(checkpoint_dir.clone())
            .upload(
                &intent,
                "channel",
                SecretString::new("unused"),
                no_refresh(),
                cancellation,
                Arc::new(|_| panic!("cancelled upload must not start")),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "UPLOAD_CANCELLED");
        assert!(!checkpoint_dir.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checkpoint_debug_redacts_session_url() {
        let checkpoint = UploadCheckpoint {
            file_path: PathBuf::from("/tmp/video.mp4"),
            file_size: 10,
            modified_unix_nanos: 20,
            uploaded_offset: 0,
            session_url: "https://www.googleapis.com/secret-session".into(),
            metadata_hash: "meta".into(),
            channel_hash: "channel".into(),
        };
        assert!(!format!("{checkpoint:?}").contains("secret-session"));
    }

    #[test]
    fn terminal_response_uses_server_truth_and_allows_unknown_privacy() {
        let private =
            parse_upload_result(br#"{"id":"video-1","status":{"privacyStatus":"private"}}"#)
                .unwrap();
        assert_eq!(private.privacy_status, Some(PrivacyStatus::Private));
        let unknown = parse_upload_result(br#"{"id":"video-2"}"#).unwrap();
        assert_eq!(unknown.privacy_status, None);
        assert_eq!(
            unknown.youtube_url,
            "https://www.youtube.com/watch?v=video-2"
        );
    }

    #[test]
    fn checkpoint_with_private_session_url_is_always_owner_only() {
        // Production mutation caught: writing a resumable session URL with default
        // world-readable permissions or preserving unsafe permissions on replacement.
        let root = temp_path("permissions");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
        let path = root.join("job-1.json");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        let checkpoint = UploadCheckpoint {
            file_path: PathBuf::from("/tmp/video.mp4"),
            file_size: 10,
            modified_unix_nanos: 20,
            uploaded_offset: 0,
            session_url: "https://www.googleapis.com/upload/session-secret".into(),
            metadata_hash: "meta".into(),
            channel_hash: "channel".into(),
        };

        save_checkpoint(&path, &checkpoint).unwrap();

        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn cancellation_after_308_resumes_from_the_persisted_server_offset() {
        // Production mutation caught: recreating a session or retransmitting byte zero
        // after a cancelled upload has persisted a server-confirmed offset.
        for pause in [false, true] {
            let root = temp_path("resume");
            fs::create_dir_all(&root).unwrap();
            let intent = upload_intent(&root, CHUNK_SIZE as usize + 3, "resume-job");
            let server = MockUploadServer::serve(|session| {
                vec![
                    MockReply {
                        status: "200 OK",
                        headers: vec![("Location", session.to_string())],
                        body: "",
                    },
                    MockReply {
                        status: "308 Permanent Redirect",
                        headers: vec![("Range", format!("bytes=0-{}", CHUNK_SIZE - 1))],
                        body: "",
                    },
                    MockReply {
                        status: "308 Permanent Redirect",
                        headers: vec![("Range", format!("bytes=0-{}", CHUNK_SIZE - 1))],
                        body: "",
                    },
                    MockReply {
                        status: "200 OK",
                        headers: vec![],
                        body: r#"{"id":"video-resumed","status":{"privacyStatus":"private"}}"#,
                    },
                ]
            });
            let checkpoint_dir = root.join("checkpoints");
            let uploader = server.uploader(checkpoint_dir.clone());
            let cancellation = UploadCancellationToken::default();
            let cancel_on_checkpoint = cancellation.clone();
            let progress: Arc<dyn Fn(UploadProgressEvent) + Send + Sync> = Arc::new(move |event| {
                if event.uploaded_bytes == CHUNK_SIZE {
                    if pause {
                        cancel_on_checkpoint.pause();
                    } else {
                        cancel_on_checkpoint.cancel();
                    }
                }
            });

            let error = uploader
                .upload(
                    &intent,
                    "UC_CHANNEL",
                    SecretString::new("initial-token"),
                    no_refresh(),
                    cancellation,
                    progress,
                )
                .await
                .unwrap_err();
            assert_eq!(
                error.code,
                if pause {
                    "UPLOAD_PAUSED"
                } else {
                    "UPLOAD_CANCELLED"
                }
            );
            assert!(checkpoint_dir.join("resume-job.json").is_file());

            let result = uploader
                .upload(
                    &intent,
                    "UC_CHANNEL",
                    SecretString::new("initial-token"),
                    no_refresh(),
                    UploadCancellationToken::default(),
                    Arc::new(|_| {}),
                )
                .await
                .unwrap();

            assert_eq!(result.video_id, "video-resumed");
            assert!(!checkpoint_dir.join("resume-job.json").exists());
            let requests = server.requests();
            assert_eq!(requests.len(), 4);
            assert!(requests[0].line.starts_with("POST /videos?"));
            assert_eq!(
                requests[1].headers.get("content-range").map(String::as_str),
                Some("bytes 0-8388607/8388611")
            );
            assert_eq!(requests[1].body_bytes, CHUNK_SIZE as usize);
            assert_eq!(
                requests[3].headers.get("content-range").map(String::as_str),
                Some("bytes 8388608-8388610/8388611")
            );
            assert_eq!(requests[3].body_bytes, 3);
            assert_eq!(
                requests[2].headers.get("content-range").map(String::as_str),
                Some("bytes */8388611")
            );
            assert_eq!(requests[2].body_bytes, 0);
            let _ = fs::remove_dir_all(root);
        }
    }

    fn seed_checkpoint(root: &Path, intent: &UploadIntent, session: &str, offset: u64) -> PathBuf {
        let dir = root.join("checkpoints");
        fs::create_dir_all(&dir).unwrap();
        let source = snapshot_source(&intent.file_path).unwrap();
        save_checkpoint(
            &dir.join(format!("{}.json", intent.job_id)),
            &UploadCheckpoint {
                file_path: source.path,
                file_size: source.size,
                modified_unix_nanos: source.modified_unix_nanos,
                uploaded_offset: offset,
                session_url: session.into(),
                metadata_hash: intent_hash(intent),
                channel_hash: hash_text("UC_CHANNEL"),
            },
        )
        .unwrap();
        dir
    }

    #[tokio::test]
    async fn resume_recovers_a_lost_completion_response_without_resending_video() {
        let root = temp_path("completed-response-lost");
        fs::create_dir_all(&root).unwrap();
        let intent = upload_intent(&root, 16, "completed-job");
        let server = MockUploadServer::serve(|_| {
            vec![MockReply {
                status: "201 Created",
                headers: vec![],
                body: r#"{"id":"already-uploaded"}"#,
            }]
        });
        let dir = seed_checkpoint(
            &root,
            &intent,
            server.endpoint.join("/session").unwrap().as_str(),
            0,
        );
        let result = server
            .uploader(dir.clone())
            .upload(
                &intent,
                "UC_CHANNEL",
                SecretString::new("test"),
                no_refresh(),
                UploadCancellationToken::default(),
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        assert_eq!(result.video_id, "already-uploaded");
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].line.starts_with("PUT /session"));
        assert_eq!(requests[0].body_bytes, 0);
        assert_eq!(requests[0].headers["content-range"], "bytes */16");
        assert!(!dir.join("completed-job.json").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn retry_uses_server_offset_after_a_transient_status_query_failure() {
        let root = temp_path("server-offset");
        fs::create_dir_all(&root).unwrap();
        let intent = upload_intent(&root, 16, "offset-job");
        let server = MockUploadServer::serve(|_| {
            vec![
                MockReply {
                    status: "503 Service Unavailable",
                    headers: vec![],
                    body: "",
                },
                MockReply {
                    status: "308 Permanent Redirect",
                    headers: vec![("Range", "bytes=0-4".into())],
                    body: "",
                },
                MockReply {
                    status: "200 OK",
                    headers: vec![],
                    body: r#"{"id":"recovered"}"#,
                },
            ]
        });
        let dir = seed_checkpoint(
            &root,
            &intent,
            server.endpoint.join("/session").unwrap().as_str(),
            2,
        );
        let result = server
            .uploader(dir)
            .upload(
                &intent,
                "UC_CHANNEL",
                SecretString::new("test"),
                no_refresh(),
                UploadCancellationToken::default(),
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        assert_eq!(result.video_id, "recovered");
        let requests = server.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].body_bytes, 0);
        assert_eq!(requests[1].body_bytes, 0);
        assert_eq!(requests[2].headers["content-range"], "bytes 5-15/16");
        assert_eq!(requests[2].body_bytes, 11);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn pause_interrupts_retry_backoff_and_keeps_the_session() {
        let root = temp_path("pause-retry");
        fs::create_dir_all(&root).unwrap();
        let intent = upload_intent(&root, 16, "pause-job");
        let server = MockUploadServer::serve(|_| {
            vec![MockReply {
                status: "503 Service Unavailable",
                headers: vec![],
                body: "",
            }]
        });
        let dir = seed_checkpoint(
            &root,
            &intent,
            server.endpoint.join("/session").unwrap().as_str(),
            2,
        );
        let stop = UploadCancellationToken::default();
        let pause = stop.clone();
        let progress: Arc<dyn Fn(UploadProgressEvent) + Send + Sync> = Arc::new(move |event| {
            if event.status == YouTubeJobStatus::WaitingToRetry {
                pause.pause();
            }
        });
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            server.uploader(dir.clone()).upload(
                &intent,
                "UC_CHANNEL",
                SecretString::new("test"),
                no_refresh(),
                stop,
                progress,
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error.code, "UPLOAD_PAUSED");
        assert_eq!(server.requests().len(), 1);
        assert!(dir.join("pause-job.json").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn pause_interrupts_an_inflight_network_request_without_waiting_for_timeout() {
        use tokio::io::AsyncReadExt;
        let root = temp_path("pause-inflight");
        fs::create_dir_all(&root).unwrap();
        let intent = upload_intent(&root, 16, "inflight-job");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Url::parse(&format!(
            "http://{}/session",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let dir = seed_checkpoint(&root, &intent, endpoint.as_str(), 2);
        let (received, waiting) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            socket.read(&mut bytes).await.unwrap();
            received.send(()).unwrap();
            std::future::pending::<()>().await;
            drop(socket);
        });
        let uploader = ResumableUploader {
            client: Client::builder().no_proxy().build().unwrap(),
            endpoint,
            checkpoint_dir: dir.clone(),
        };
        let stop = UploadCancellationToken::default();
        let control = stop.clone();
        let task = tokio::spawn(async move {
            uploader
                .upload(
                    &intent,
                    "UC_CHANNEL",
                    SecretString::new("test"),
                    no_refresh(),
                    stop,
                    Arc::new(|_| {}),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .unwrap()
            .unwrap();
        control.pause();
        let error = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.code, "UPLOAD_PAUSED");
        assert!(dir.join("inflight-job.json").exists());
        server.abort();
        let _ = server.await;
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn one_unauthorized_session_response_refreshes_once_and_reuses_the_token() {
        // Production mutation caught: looping refreshes or retrying the session request
        // with the expired bearer token.
        let root = temp_path("refresh");
        fs::create_dir_all(&root).unwrap();
        let intent = upload_intent(&root, 4, "refresh-job");
        let server = MockUploadServer::serve(|session| {
            vec![
                MockReply {
                    status: "401 Unauthorized",
                    headers: vec![],
                    body: "",
                },
                MockReply {
                    status: "200 OK",
                    headers: vec![("Location", session.to_string())],
                    body: "",
                },
                MockReply {
                    status: "200 OK",
                    headers: vec![],
                    body: r#"{"id":"video-refreshed","status":{"privacyStatus":"unlisted"}}"#,
                },
            ]
        });
        let refreshes = Arc::new(AtomicUsize::new(0));
        let refresh: RefreshCallback = Arc::new({
            let refreshes = refreshes.clone();
            move || {
                refreshes.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(SecretString::new("refreshed-token")) })
            }
        });

        let result = server
            .uploader(root.join("checkpoints"))
            .upload(
                &intent,
                "UC_CHANNEL",
                SecretString::new("expired-token"),
                refresh,
                UploadCancellationToken::default(),
                Arc::new(|_| {}),
            )
            .await
            .unwrap();

        assert_eq!(result.video_id, "video-refreshed");
        assert_eq!(refreshes.load(Ordering::SeqCst), 1);
        let requests = server.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[0].headers.get("authorization").map(String::as_str),
            Some("Bearer expired-token")
        );
        for request in &requests[1..] {
            assert_eq!(
                request.headers.get("authorization").map(String::as_str),
                Some("Bearer refreshed-token")
            );
        }
        let _ = fs::remove_dir_all(root);
    }
}
