use super::config::SecretString;
use crate::{
    media::{MediaJob, MediaJobKind, MediaJobOutputKind, MediaJobScope, MediaJobStatus},
    AppError,
};
use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

const MAX_BYTES: u64 = 100 * 1024 * 1024;
const LIST_URL: &str = "https://www.googleapis.com/youtube/v3/captions";
const UPLOAD_URL: &str = "https://www.googleapis.com/upload/youtube/v3/captions";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleRequest {
    // None requests automatic matching; resolved before persisting an upload.
    pub path: Option<PathBuf>,
    pub language: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum SubtitleState {
    #[default]
    Skipped,
    Pending,
    Submitted,
    Failed,
}

impl SubtitleRequest {
    pub fn validate(&self) -> Result<(), AppError> {
        let mut parts = self.language.split('-');
        if !parts.next().is_some_and(|v| {
            (2..=3).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphabetic())
        }) || self.language.len() > 35
            || parts.any(|v| {
                v.is_empty() || v.len() > 8 || !v.bytes().all(|b| b.is_ascii_alphanumeric())
            })
        {
            return Err(AppError::new(
                "SUBTITLE_LANGUAGE_INVALID",
                "请选择有效的字幕语言",
            ));
        }
        if let Some(path) = &self.path {
            read_srt(path)?;
        }
        Ok(())
    }
}

fn invalid() -> AppError {
    AppError::new(
        "SUBTITLE_INVALID",
        "字幕必须是有效的 UTF-8 SRT 文件，包含时间轴，且不超过 100 MB",
    )
}

fn timestamp(value: &str) -> Option<u64> {
    let parts: Vec<_> = value.split([':', ',']).collect();
    if parts.len() != 4
        || parts[0].len() < 2
        || parts[1].len() != 2
        || parts[2].len() != 2
        || parts[3].len() != 3
        || parts.iter().any(|v| !v.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let h: u64 = parts[0].parse().ok()?;
    let m: u64 = parts[1].parse().ok()?;
    let s: u64 = parts[2].parse().ok()?;
    let ms: u64 = parts[3].parse().ok()?;
    if h > 9999 || m > 59 || s > 59 {
        return None;
    }
    Some(((h * 60 + m) * 60 + s) * 1000 + ms)
}

pub fn read_srt(path: &Path) -> Result<Vec<u8>, AppError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| invalid())?;
    if !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_BYTES
        || !path
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("srt"))
    {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| invalid())?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid());
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| invalid())?
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n");
    let mut cues = 0;
    for block in text.trim().split("\n\n").filter(|b| !b.trim().is_empty()) {
        let mut lines = block.trim().lines();
        if lines
            .next()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .is_none()
        {
            return Err(invalid());
        }
        let (start, end) = lines
            .next()
            .and_then(|v| v.split_once(" --> "))
            .ok_or_else(invalid)?;
        let start = timestamp(start.trim()).ok_or_else(invalid)?;
        let end = timestamp(end.trim()).ok_or_else(invalid)?;
        if start >= end || !lines.any(|v| !v.trim().is_empty()) {
            return Err(invalid());
        }
        cues += 1;
    }
    if cues == 0 {
        return Err(invalid());
    }
    Ok(bytes)
}

fn current_input(input: &crate::media::MergeInput) -> bool {
    fs::metadata(&input.path).is_ok_and(|m| {
        m.len() == input.size
            && m.modified()
                .ok()
                .and_then(|v| v.duration_since(UNIX_EPOCH).ok())
                .is_some_and(|v| v.as_nanos() == input.modified_unix_nanos)
    })
}

// A separated video shares the original timeline only while its published result is unchanged.
fn current_separated_output(path: &Path, input: &crate::media::MergeInput) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let Ok(file) = fs::File::open(parent.join("result.json")) else {
        return false;
    };
    let mut bytes = Vec::new();
    if file.take(64 * 1024).read_to_end(&mut bytes).is_err() {
        return false;
    }
    let Ok(record) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    let recorded_input = record
        .pointer("/identity/source/input")
        .cloned()
        .and_then(|v| serde_json::from_value::<crate::media::MergeInput>(v).ok());
    if recorded_input.as_ref() != Some(input)
        || record
            .pointer("/identity/source/scope")
            .and_then(Value::as_str)
            != Some("merged")
    {
        return false;
    }
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|m| m.as_nanos());
    record
        .get("files")
        .and_then(Value::as_array)
        .is_some_and(|files| {
            files.iter().any(|entry| {
                entry.get("name").and_then(Value::as_str)
                    == path.file_name().and_then(|v| v.to_str())
                    && entry.get("size").and_then(Value::as_u64) == Some(metadata.len())
                    && entry
                        .get("modified_unix_nanos")
                        .and_then(Value::as_u64)
                        .map(u128::from)
                        == modified
            })
        })
}

/// Match a verified whole-video extraction, never episode subtitles or a title-only match.
pub fn find_subtitle(jobs: &[MediaJob], source: &Path) -> Option<PathBuf> {
    let source = fs::canonicalize(source).ok()?;
    let mut inputs = vec![source.clone()];
    for job in jobs.iter().filter(|j| {
        j.status == MediaJobStatus::Completed && j.kind == MediaJobKind::SeparateBackgroundMusic
    }) {
        let Some(request) = &job.ai_request else {
            continue;
        };
        if request.scope != MediaJobScope::Merged
            || request.inputs.len() != 1
            || !current_input(&request.inputs[0])
        {
            continue;
        }
        if job.outputs.iter().any(|o| {
            o.kind == MediaJobOutputKind::NoBackgroundMusicVideo
                && fs::canonicalize(&o.path).is_ok_and(|p| p == source)
                && current_separated_output(&o.path, &request.inputs[0])
        }) {
            if let Ok(path) = fs::canonicalize(&request.inputs[0].path) {
                inputs.push(path);
            }
        }
    }
    for job in jobs.iter().rev().filter(|j| {
        j.status == MediaJobStatus::Completed && j.kind == MediaJobKind::ExtractSubtitles
    }) {
        let Some(request) = &job.ai_request else {
            continue;
        };
        if request.scope != MediaJobScope::Merged
            || request.inputs.len() != 1
            || !current_input(&request.inputs[0])
        {
            continue;
        }
        if !fs::canonicalize(&request.inputs[0].path).is_ok_and(|p| inputs.contains(&p)) {
            continue;
        }
        if let Some(output) = job
            .outputs
            .iter()
            .find(|o| o.kind == MediaJobOutputKind::Subtitles && read_srt(&o.path).is_ok())
        {
            return Some(output.path.clone());
        }
    }
    None
}

async fn response_json(response: reqwest::Response) -> Result<Value, AppError> {
    let status = response.status();
    let body = response.json::<Value>().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    let reason = body
        .pointer("/error/errors/0/reason")
        .and_then(Value::as_str)
        .unwrap_or("");
    let (code, message) = match (status, reason) {
        (_, "quotaExceeded" | "dailyLimitExceeded") => {
            ("SUBTITLE_QUOTA", "YouTube 字幕配额已用完，请稍后仅重试字幕")
        }
        (StatusCode::UNAUTHORIZED, _) => (
            "AUTH_REQUIRED",
            "请重新授权此任务的 YouTube 频道后，仅重试字幕",
        ),
        (StatusCode::FORBIDDEN, "accessNotConfigured" | "serviceDisabled") => (
            "YOUTUBE_API_NOT_ENABLED",
            "请在 Google Cloud 启用 YouTube Data API v3",
        ),
        (StatusCode::FORBIDDEN, _) => (
            "SUBTITLE_SCOPE_REQUIRED",
            "缺少字幕管理权限，请到设置重新授权此任务的 YouTube 频道并勾选权限，再仅重试字幕",
        ),
        (StatusCode::TOO_MANY_REQUESTS, _) => (
            "SUBTITLE_RATE_LIMITED",
            "YouTube 字幕接口限流，请稍后仅重试字幕",
        ),
        _ => (
            "SUBTITLE_UPLOAD_FAILED",
            "YouTube 字幕上传失败，请检查字幕文件后仅重试字幕",
        ),
    };
    Err(AppError::new(code, message))
}
fn network_error(_: reqwest::Error) -> AppError {
    AppError::new(
        "SUBTITLE_NETWORK",
        "字幕上传连接失败，可仅重试字幕；会先检查是否已上传",
    )
}

pub async fn upload_subtitle(
    video_id: &str,
    request: &SubtitleRequest,
    token: &SecretString,
) -> Result<(), AppError> {
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(network_error)?;
    upload_with(&client, LIST_URL, UPLOAD_URL, video_id, request, token).await
}

async fn upload_with(
    client: &Client,
    list_url: &str,
    upload_url: &str,
    video_id: &str,
    request: &SubtitleRequest,
    token: &SecretString,
) -> Result<(), AppError> {
    request.validate()?;
    let bytes = read_srt(request.path.as_deref().ok_or_else(invalid)?)?;
    if video_id.is_empty()
        || video_id.len() > 64
        || !video_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(AppError::new(
            "UPLOAD_VIDEO_ID_MISSING",
            "YouTube 视频 ID 无效",
        ));
    }
    // Stable content identity recovers a lost response without creating a second track.
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let name = format!("字幕 {}", &digest[..16]);
    let list = response_json(
        client
            .get(list_url)
            .query(&[("part", "snippet"), ("videoId", video_id)])
            .bearer_auth(token.expose_secret())
            .send()
            .await
            .map_err(network_error)?,
    )
    .await?;
    let items = list.get("items").and_then(Value::as_array).ok_or_else(|| {
        AppError::new(
            "SUBTITLE_RESPONSE_INVALID",
            "无法确认已有字幕，请稍后仅重试字幕",
        )
    })?;
    let existing = items.iter().find(|item| {
        item.pointer("/snippet/name").and_then(Value::as_str) == Some(&name)
            && item.pointer("/snippet/language").and_then(Value::as_str) == Some(&request.language)
    });
    if existing.is_some_and(|item| {
        matches!(
            item.pointer("/snippet/status").and_then(Value::as_str),
            Some("serving" | "syncing")
        )
    }) {
        return Ok(());
    }
    let metadata = if let Some(item) = existing {
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(invalid)?;
        json!({"id": id, "snippet": {"isDraft": false}})
    } else {
        json!({"snippet": {"videoId": video_id, "language": request.language, "name": name, "isDraft": false}})
    };
    let boundary = format!("hongguo_{}", digest);
    let mut body = format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
    body.extend(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").bytes());
    let response = client
        .request(
            if existing.is_some() {
                Method::PUT
            } else {
                Method::POST
            },
            upload_url,
        )
        .query(&[("part", "snippet"), ("uploadType", "multipart")])
        .bearer_auth(token.expose_secret())
        .header(
            "Content-Type",
            format!("multipart/related; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .map_err(network_error)?;
    let result = response_json(response).await?;
    if result
        .get("id")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(AppError::new(
            "SUBTITLE_RESPONSE_INVALID",
            "字幕响应不完整，可仅重试字幕确认结果",
        ));
    }
    if result.pointer("/snippet/status").and_then(Value::as_str) == Some("failed") {
        return Err(AppError::new(
            "SUBTITLE_PROCESSING_FAILED",
            "YouTube 无法处理此字幕，请检查 SRT 后仅重试字幕",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    const SRT: &str = "1\n00:00:00,000 --> 00:00:02,500\n你好，世界\n\n2\n00:00:02,500 --> 00:00:05,000\n第二句\n";
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "hongguo-caption-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn request(&self) -> SubtitleRequest {
            let path = self.0.join("字幕.srt");
            fs::write(&path, SRT).unwrap();
            SubtitleRequest {
                path: Some(path),
                language: "zh-Hans".into(),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn accepts_utf8_bom_and_crlf_but_rejects_invalid_timing_and_encoding() {
        let fixture = Fixture::new();
        let request = fixture.request();
        let path = request.path.unwrap();
        fs::write(&path, format!("\u{feff}{}", SRT.replace('\n', "\r\n"))).unwrap();
        assert!(read_srt(&path).is_ok());
        for text in [
            "plain transcript",
            "1\n00:99:00,000 --> 00:00:02,500\n正文",
            "1\n00:00:05,000 --> 00:00:02,500\n正文",
            "1\n00:00:00,000 --> 00:00:02,500\n",
        ] {
            fs::write(&path, text).unwrap();
            assert!(read_srt(&path).is_err());
        }
        fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(read_srt(&path).is_err());
        assert!(SubtitleRequest {
            path: None,
            language: "zh\r\nInjected".into()
        }
        .validate()
        .is_err());
    }

    fn extraction(source: &Path, srt: &Path, scope: &str) -> MediaJob {
        let metadata = fs::metadata(source).unwrap();
        serde_json::from_value(json!({"id":"subtitles", "dedupeKey":"test", "kind":"extractSubtitles", "status":"completed", "stage":"done", "percent":100,
            "inputs":[], "outputPath":null, "errorCode":null, "errorMessage":null,
            "outputs":[{"episodeIndex":1, "kind":"subtitles", "path":srt}],
            "aiRequest":{"bookId":"book", "title":"剧", "kind":"extractSubtitles", "scope":scope, "seriesRoot":source.parent(), "model":"small", "dedupeKey":"test",
                "inputs":[{"episodeIndex":1,"path":source,"size":metadata.len(),"modifiedUnixNanos":metadata.modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_nanos()}]}
        })).unwrap()
    }
    #[test]
    fn matching_skips_episode_wrong_video_missing_and_stale_results() {
        let f = Fixture::new();
        let request = f.request();
        let srt = request.path.unwrap();
        let video = f.0.join("merged.mp4");
        fs::write(&video, b"merged video").unwrap();
        let other = f.0.join("other.mp4");
        fs::write(&other, b"another video").unwrap();
        assert!(find_subtitle(&[extraction(&video, &srt, "episodes")], &video).is_none());
        assert!(find_subtitle(&[extraction(&other, &srt, "merged")], &video).is_none());
        let job = extraction(&video, &srt, "merged");
        assert_eq!(
            find_subtitle(std::slice::from_ref(&job), &video),
            Some(srt.clone())
        );
        fs::write(&video, b"replaced video has new timing").unwrap();
        assert!(find_subtitle(&[job], &video).is_none());
        let job = extraction(&video, &srt, "merged");
        fs::remove_file(&srt).unwrap();
        assert!(find_subtitle(&[job], &video).is_none());
    }

    #[test]
    fn separated_video_reuses_original_subtitles_only_with_unchanged_result_record() {
        let f = Fixture::new();
        let request = f.request();
        let srt = request.path.unwrap();
        let video = f.0.join("merged.mp4");
        fs::write(&video, b"merged video").unwrap();
        let clean = f.0.join("clean.mp4");
        fs::write(&clean, b"clean video").unwrap();
        let subtitles = extraction(&video, &srt, "merged");
        let mut separation = subtitles.clone();
        separation.kind = MediaJobKind::SeparateBackgroundMusic;
        separation.ai_request.as_mut().unwrap().kind = MediaJobKind::SeparateBackgroundMusic;
        separation.outputs[0].kind = MediaJobOutputKind::NoBackgroundMusicVideo;
        separation.outputs[0].path = clean.clone();
        let jobs = [subtitles, separation];
        assert!(find_subtitle(&jobs, &clean).is_none());
        let metadata = fs::metadata(&clean).unwrap();
        fs::write(f.0.join("result.json"), serde_json::to_vec(&json!({
            "identity":{"source":{"scope":"merged","input":jobs[1].ai_request.as_ref().unwrap().inputs[0]}},
            "files":[{"name":"clean.mp4","size":metadata.len(),"modified_unix_nanos":metadata.modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_nanos()}]
        })).unwrap()).unwrap();
        assert_eq!(find_subtitle(&jobs, &clean), Some(srt));
        fs::write(&clean, b"trimmed clean video changes timing").unwrap();
        assert!(find_subtitle(&jobs, &clean).is_none());
    }

    async fn server(
        responses: Vec<(u16, Value)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (status, response) in responses {
                let (mut stream, _) =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buffer = [0u8; 4096];
                    let size =
                        tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
                            .await
                            .unwrap()
                            .unwrap();
                    assert!(size > 0);
                    bytes.extend_from_slice(&buffer[..size]);
                    if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(str::trim)
                                    .and_then(|v| v.parse().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8(bytes).unwrap());
                let body = response.to_string();
                let reply = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
            requests
        });
        (url, task)
    }
    #[tokio::test]
    async fn uploads_real_timed_utf8_multipart_with_metadata() {
        let f = Fixture::new();
        let request = f.request();
        let (url, server) = server(vec![
            (200, json!({"items":[]})),
            (
                200,
                json!({"id":"caption-id","snippet":{"status":"syncing"}}),
            ),
        ])
        .await;
        upload_with(
            &Client::builder().no_proxy().build().unwrap(),
            &format!("{url}/captions"),
            &format!("{url}/upload/captions"),
            "video_1",
            &request,
            &SecretString::new("test-token"),
        )
        .await
        .unwrap();
        let requests = server.await.unwrap();
        assert!(requests[0].starts_with("GET /captions?"));
        assert!(requests[1].starts_with("POST /upload/captions?"));
        assert!(requests[1].contains("uploadType=multipart"));
        assert!(requests[1].contains("multipart/related; boundary="));
        assert!(requests[1].contains("\"videoId\":\"video_1\""));
        assert!(requests[1].contains("\"language\":\"zh-Hans\""));
        assert!(requests[1].contains("\"isDraft\":false"));
        assert!(requests[1].contains(SRT));
        assert!(!requests[1].contains("sync=true"));
    }
    #[tokio::test]
    async fn retry_recovers_existing_track_without_inserting_again_and_updates_failed_track() {
        let f = Fixture::new();
        let request = f.request();
        let digest = format!("{:x}", Sha256::digest(SRT.as_bytes()));
        for status in ["syncing", "serving", "failed"] {
            let mut responses = vec![(
                200,
                json!({"items":[{"id":"existing", "snippet":{"name":format!("字幕 {}", &digest[..16]),"language":"zh-Hans","status":status}}]}),
            )];
            if status == "failed" {
                responses.push((
                    200,
                    json!({"id":"existing", "snippet":{"status":"syncing"}}),
                ));
            }
            let (url, server) = server(responses).await;
            upload_with(
                &Client::builder().no_proxy().build().unwrap(),
                &url,
                &url,
                "video_1",
                &request,
                &SecretString::new("test-token"),
            )
            .await
            .unwrap();
            let requests = server.await.unwrap();
            if status == "failed" {
                assert!(requests[1].starts_with("PUT "));
                assert!(requests[1].contains("\"id\":\"existing\""));
            } else {
                assert_eq!(requests.len(), 1);
            }
        }
    }
    #[tokio::test]
    async fn old_permission_fails_without_uploading_and_does_not_expose_response() {
        let f = Fixture::new();
        let (url, server) = server(vec![(403,json!({"error":{"message":"secret body","errors":[{"reason":"insufficientPermissions"}]}}))]).await;
        let error = upload_with(
            &Client::builder().no_proxy().build().unwrap(),
            &url,
            &url,
            "video_1",
            &f.request(),
            &SecretString::new("test-token"),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "SUBTITLE_SCOPE_REQUIRED");
        assert!(!error.message.contains("secret"));
        assert_eq!(server.await.unwrap().len(), 1);
    }
}
