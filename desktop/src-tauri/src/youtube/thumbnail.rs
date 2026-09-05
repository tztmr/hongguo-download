use super::config::SecretString;
use crate::{media::MediaTools, AppError};
use reqwest::{Client, StatusCode};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};
use url::Url;

const MAX_THUMBNAIL_BYTES: u64 = 2 * 1024 * 1024;
const THUMBNAIL_ENDPOINT: &str = "https://www.googleapis.com/upload/youtube/v3/thumbnails/set";

pub fn prepare_thumbnail(
    source: &Path,
    tools: &MediaTools,
    temp_dir: &Path,
) -> Result<PathBuf, AppError> {
    let bytes = fs::read(source).map_err(|error| {
        AppError::with_cause("THUMBNAIL_INVALID", "无法读取封面", error.to_string())
    })?;
    if bytes.len() as u64 <= MAX_THUMBNAIL_BYTES && (is_jpeg(&bytes) || is_png(&bytes)) {
        let extension = if is_png(&bytes) { "png" } else { "jpg" };
        let output = temp_dir.join(format!("youtube-thumbnail.{extension}"));
        fs::write(&output, bytes).map_err(thumbnail_io)?;
        return Ok(output);
    }
    for quality in [3, 6, 10] {
        let output = temp_dir.join(format!("youtube-thumbnail-{quality}.jpg"));
        let status = tools
            .ffmpeg_command()
            .args(["-nostdin", "-v", "error", "-y", "-i"])
            .arg(source)
            .args(["-frames:v", "1", "-q:v"])
            .arg(quality.to_string())
            .arg(&output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| {
                AppError::with_cause(
                    "THUMBNAIL_CONVERT_FAILED",
                    "封面转换失败",
                    error.to_string(),
                )
            })?;
        if status.success()
            && fs::metadata(&output)
                .is_ok_and(|value| value.len() > 0 && value.len() <= MAX_THUMBNAIL_BYTES)
        {
            return Ok(output);
        }
    }
    Err(AppError::new(
        "THUMBNAIL_TOO_LARGE",
        "封面无法压缩到 2 MB 以内",
    ))
}

pub async fn set_thumbnail(
    client: &Client,
    video_id: &str,
    path: &Path,
    token: &SecretString,
) -> Result<(), AppError> {
    if video_id.is_empty() || video_id.len() > 64 {
        return Err(AppError::new("THUMBNAIL_INVALID", "YouTube 视频 ID 无效"));
    }
    let bytes = fs::read(path).map_err(thumbnail_io)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_THUMBNAIL_BYTES {
        return Err(AppError::new("THUMBNAIL_INVALID", "YouTube 封面无效"));
    }
    let content_type = if is_png(&bytes) {
        "image/png"
    } else if is_jpeg(&bytes) {
        "image/jpeg"
    } else {
        return Err(AppError::new(
            "THUMBNAIL_INVALID",
            "YouTube 封面必须为 JPEG 或 PNG",
        ));
    };
    let endpoint = Url::parse(THUMBNAIL_ENDPOINT).expect("fixed thumbnail endpoint is valid");
    let response = client
        .post(endpoint)
        .query(&[("videoId", video_id)])
        .bearer_auth(token.expose_secret())
        .header("Content-Type", content_type)
        .body(bytes)
        .send()
        .await
        .map_err(|_| AppError::new("THUMBNAIL_UPLOAD_FAILED", "YouTube 封面上传失败"))?;
    match response.status() {
        status if status.is_success() => Ok(()),
        StatusCode::TOO_MANY_REQUESTS => Err(AppError::new(
            "THUMBNAIL_RATE_LIMITED",
            "YouTube 封面接口限流，可稍后重试",
        )),
        StatusCode::FORBIDDEN => Err(AppError::new(
            "THUMBNAIL_FORBIDDEN",
            "YouTube 频道暂无自定义封面权限",
        )),
        StatusCode::UNAUTHORIZED => {
            Err(AppError::new("AUTH_REQUIRED", "需要重新授权 YouTube 频道"))
        }
        _ => Err(AppError::new(
            "THUMBNAIL_UPLOAD_FAILED",
            "YouTube 封面上传失败",
        )),
    }
}

fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xff, 0xd8, 0xff])
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

fn thumbnail_io(error: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "THUMBNAIL_IO",
        "YouTube 封面文件操作失败",
        error.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_jpeg_and_png_signatures() {
        assert!(is_jpeg(&[0xff, 0xd8, 0xff, 0x00]));
        assert!(is_png(b"\x89PNG\r\n\x1a\nrest"));
        assert!(!is_jpeg(b"GIF89a"));
        assert!(!is_png(b"GIF89a"));
    }
}
