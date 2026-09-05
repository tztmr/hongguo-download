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
use std::{
    fs::{self, File, OpenOptions},
    future::Future,
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, UNIX_EPOCH},
};
use tokio::time::sleep;
use url::Url;

const UPLOAD_ENDPOINT: &str = "https://www.googleapis.com/upload/youtube/v3/videos";
pub const CHUNK_SIZE: u64 = 8 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct UploadCancellationToken(Arc<AtomicBool>);

impl UploadCancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
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
            client: Client::new(),
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
        intent.validate()?;
        let source = snapshot_source(&intent.file_path)?;
        fs::create_dir_all(&self.checkpoint_dir).map_err(upload_io)?;
        let checkpoint_path = self.checkpoint_dir.join(format!("{}.json", intent.job_id));
        emit(
            &progress,
            YouTubeJobStatus::PreparingAuthorization,
            0,
            source.size,
        );
        let metadata_hash = intent_hash(intent);
        let channel_hash = hash_text(channel_id);
        let mut token = initial_token;
        let mut refreshed = false;
        let mut checkpoint = match load_checkpoint(&checkpoint_path)? {
            Some(value) => {
                validate_checkpoint(&value, &source, &metadata_hash, &channel_hash)?;
                value
            }
            None => {
                emit(&progress, YouTubeJobStatus::CreatingSession, 0, source.size);
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
        loop {
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
            let mut response = self
                .send_chunk(&session, start, end, source.size, bytes.clone(), &token)
                .await;
            if response
                .as_ref()
                .is_ok_and(|value| value.status() == StatusCode::UNAUTHORIZED)
                && !refreshed
            {
                token = refresh().await?;
                refreshed = true;
                response = self
                    .send_chunk(&session, start, end, source.size, bytes, &token)
                    .await;
            }
            match response {
                Ok(value) if value.status().as_u16() == 308 => {
                    checkpoint.uploaded_offset = next_offset(&value)?;
                    if checkpoint.uploaded_offset > source.size
                        || checkpoint.uploaded_offset <= start
                    {
                        return Err(AppError::new(
                            "UPLOAD_SERVER_OFFSET_INVALID",
                            "YouTube 返回的上传位置无效",
                        ));
                    }
                    save_checkpoint(&checkpoint_path, &checkpoint)?;
                    emit(
                        &progress,
                        YouTubeJobStatus::Uploading,
                        checkpoint.uploaded_offset,
                        source.size,
                    );
                }
                Ok(value) if value.status().is_success() => {
                    emit(
                        &progress,
                        YouTubeJobStatus::Processing,
                        source.size,
                        source.size,
                    );
                    let bytes = value.bytes().await.map_err(|_| {
                        AppError::new("UPLOAD_RESPONSE_INVALID", "YouTube 上传响应无效")
                    })?;
                    let result = parse_upload_result(&bytes)?;
                    let _ = fs::remove_file(&checkpoint_path);
                    return Ok(result);
                }
                Ok(value) if value.status() == StatusCode::NOT_FOUND => {
                    return Err(AppError::new(
                        "UPLOAD_SESSION_EXPIRED",
                        "YouTube 上传会话已过期，请明确从头开始",
                    ));
                }
                Ok(value)
                    if value.status().is_server_error()
                        || value.status() == StatusCode::TOO_MANY_REQUESTS =>
                {
                    emit(
                        &progress,
                        YouTubeJobStatus::WaitingToRetry,
                        checkpoint.uploaded_offset,
                        source.size,
                    );
                    sleep(Duration::from_secs(1)).await;
                    checkpoint.uploaded_offset =
                        self.query_offset(&session, source.size, &token).await?;
                    save_checkpoint(&checkpoint_path, &checkpoint)?;
                }
                Err(_) => {
                    emit(
                        &progress,
                        YouTubeJobStatus::WaitingToRetry,
                        checkpoint.uploaded_offset,
                        source.size,
                    );
                    sleep(Duration::from_secs(1)).await;
                    checkpoint.uploaded_offset =
                        self.query_offset(&session, source.size, &token).await?;
                    save_checkpoint(&checkpoint_path, &checkpoint)?;
                }
                Ok(value) if value.status() == StatusCode::UNAUTHORIZED => {
                    return Err(AppError::new("AUTH_REQUIRED", "需要重新授权 YouTube 频道"));
                }
                Ok(_) => {
                    return Err(AppError::new(
                        "UPLOAD_REQUEST_FAILED",
                        "YouTube 上传请求失败",
                    ))
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

    async fn query_offset(
        &self,
        session: &Url,
        total: u64,
        token: &SecretString,
    ) -> Result<u64, AppError> {
        let response = self
            .client
            .put(session.clone())
            .bearer_auth(token.expose_secret())
            .header(CONTENT_LENGTH, 0)
            .header(CONTENT_RANGE, format!("bytes */{total}"))
            .send()
            .await
            .map_err(|_| AppError::new("UPLOAD_NETWORK_FAILED", "无法恢复 YouTube 上传"))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(AppError::new(
                "UPLOAD_SESSION_EXPIRED",
                "YouTube 上传会话已过期，请明确从头开始",
            ));
        }
        if response.status().as_u16() != 308 {
            return Err(AppError::new(
                "UPLOAD_NETWORK_FAILED",
                "无法恢复 YouTube 上传",
            ));
        }
        next_offset(&response)
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
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(upload_io)?;
    let temporary = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(upload_io)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).map_err(upload_io)?;
    serde_json::to_writer(&mut file, value).map_err(upload_io)?;
    file.flush()
        .and_then(|_| file.sync_all())
        .map_err(upload_io)?;
    drop(file);
    fs::rename(temporary, path).map_err(upload_io)
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
    if cancellation.is_cancelled() {
        Err(AppError::new("UPLOAD_CANCELLED", "YouTube 上传已取消"))
    } else {
        Ok(())
    }
}

fn upload_io(error: impl std::fmt::Display) -> AppError {
    AppError::with_cause("UPLOAD_IO", "YouTube 上传文件操作失败", error.to_string())
}

#[cfg(test)]
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
                client: Client::new(),
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
                cancel_on_checkpoint.cancel();
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
        assert_eq!(error.code, "UPLOAD_CANCELLED");
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
        assert_eq!(requests.len(), 3);
        assert!(requests[0].line.starts_with("POST /videos?"));
        assert_eq!(
            requests[1].headers.get("content-range").map(String::as_str),
            Some("bytes 0-8388607/8388611")
        );
        assert_eq!(requests[1].body_bytes, CHUNK_SIZE as usize);
        assert_eq!(
            requests[2].headers.get("content-range").map(String::as_str),
            Some("bytes 8388608-8388610/8388611")
        );
        assert_eq!(requests[2].body_bytes, 3);
        let _ = fs::remove_dir_all(root);
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
