use super::config::SecretString;
use crate::{media::MediaTools, AppError};
use reqwest::{Client, StatusCode};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
};
use url::Url;

const MAX_THUMBNAIL_BYTES: u64 = 2_000_000;
const TARGET_THUMBNAIL_BYTES: u64 = 1_900_000;
const THUMBNAIL_ENDPOINT: &str = "https://www.googleapis.com/upload/youtube/v3/thumbnails/set";
const MAX_REFERENCE_THUMBNAIL_BYTES: usize = 8 * 1024 * 1024;

fn reference_thumbnail_url(raw: &str) -> Result<Url, AppError> {
    let url = Url::parse(raw).map_err(|_| reference_thumbnail_invalid())?;
    let trusted_host = url
        .host_str()
        .is_some_and(|host| host.ends_with(".ytimg.com") || host == "img.youtube.com");
    if url.scheme() != "https"
        || !trusted_host
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(reference_thumbnail_invalid());
    }
    Ok(url)
}

fn reference_thumbnail_invalid() -> AppError {
    AppError::new(
        "THUMBNAIL_INVALID",
        "YouTube 原封面地址或图片无效，或图片超过 8 MB",
    )
}

fn reference_thumbnail_unavailable() -> AppError {
    AppError::new(
        "YOUTUBE_THUMBNAIL_FETCH_FAILED",
        "无法读取 YouTube 原封面，请检查系统代理或网络连接后重试",
    )
}

/// YouTube's fixed CDN hosts use the same system proxy as the other YouTube
/// requests. Do not route arbitrary image URLs here: redirects are disabled and
/// URL validation restricts this path to HTTPS on Google's thumbnail CDN.
pub async fn fetch_reference_thumbnail(raw: &str) -> Result<Vec<u8>, AppError> {
    let url = reference_thumbnail_url(raw)?;
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|_| reference_thumbnail_unavailable())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| reference_thumbnail_unavailable())?;
    read_reference_thumbnail(response).await
}

async fn read_reference_thumbnail(mut response: reqwest::Response) -> Result<Vec<u8>, AppError> {
    if !response.status().is_success() {
        return Err(reference_thumbnail_unavailable());
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_REFERENCE_THUMBNAIL_BYTES as u64)
    {
        return Err(reference_thumbnail_invalid());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| reference_thumbnail_unavailable())?
    {
        if bytes.len() + chunk.len() > MAX_REFERENCE_THUMBNAIL_BYTES {
            return Err(reference_thumbnail_invalid());
        }
        bytes.extend_from_slice(&chunk);
    }
    if crate::cover_extension(&bytes).is_none() {
        return Err(reference_thumbnail_invalid());
    }
    Ok(bytes)
}

pub fn prepare_thumbnail(
    source: &Path,
    tools: &MediaTools,
    temp_dir: &Path,
) -> Result<PathBuf, AppError> {
    let metadata = fs::metadata(source).map_err(thumbnail_io)?;
    let header = image_header(source)?;
    fs::create_dir_all(temp_dir).map_err(thumbnail_io)?;
    if metadata.len() > 0
        && metadata.len() <= TARGET_THUMBNAIL_BYTES
        && (is_jpeg(&header) || is_png(&header))
    {
        let extension = if is_png(&header) { "png" } else { "jpg" };
        let output = temp_dir.join(format!("youtube-thumbnail.{extension}"));
        fs::copy(source, &output).map_err(thumbnail_io)?;
        return Ok(output);
    }
    let mut converted = false;
    for (edge, quality, ffmpeg_quality) in [
        (1920, 85, 3),
        (1920, 70, 6),
        (1280, 80, 6),
        (960, 70, 10),
        (640, 60, 14),
    ] {
        let output = temp_dir.join(format!("youtube-thumbnail-{edge}-{quality}.jpg"));
        // The packaged FFmpeg may lack PNG decoding. Use macOS ImageIO via
        // sips first; FFmpeg remains a fallback for other supported formats.
        #[cfg(not(target_os = "macos"))]
        let mut success = false;
        #[cfg(target_os = "macos")]
        let mut success = Command::new("/usr/bin/sips")
            .args(["-s", "format", "jpeg", "-s", "formatOptions"])
            .arg(quality.to_string())
            .arg("-Z")
            .arg(edge.to_string())
            .arg(source)
            .arg("--out")
            .arg(&output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !success {
            let filter = format!("scale=w='min(iw,{edge})':h='min(ih,{edge})':force_original_aspect_ratio=decrease:force_divisible_by=2");
            success = tools
                .ffmpeg_command()
                .args(["-nostdin", "-v", "error", "-y", "-i"])
                .arg(source)
                .args(["-frames:v", "1", "-vf"])
                .arg(filter)
                .args(["-c:v", "mjpeg", "-pix_fmt", "yuvj420p", "-q:v"])
                .arg(ffmpeg_quality.to_string())
                .arg(&output)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
        }
        if success && image_header(&output).is_ok_and(|header| is_jpeg(&header)) {
            converted = true;
            if fs::metadata(&output)
                .is_ok_and(|value| value.len() > 0 && value.len() <= TARGET_THUMBNAIL_BYTES)
            {
                return Ok(output);
            }
        }
        let _ = fs::remove_file(&output);
    }
    if converted {
        Err(AppError::new(
            "THUMBNAIL_TOO_LARGE",
            "封面缩放压缩后仍超过 2 MB，请选择另一张图片",
        ))
    } else {
        Err(AppError::new(
            "THUMBNAIL_CONVERT_FAILED",
            "无法解码或转换封面图片，请确认图片完整并使用 PNG 或 JPEG",
        ))
    }
}

fn image_header(path: &Path) -> Result<Vec<u8>, AppError> {
    let mut header = vec![0; 8];
    let count = fs::File::open(path)
        .and_then(|mut file| file.read(&mut header))
        .map_err(thumbnail_io)?;
    header.truncate(count);
    Ok(header)
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
        .timeout(std::time::Duration::from_secs(60))
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

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "hongguo-thumbnail-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn unavailable_ffmpeg() -> MediaTools {
        MediaTools::from_test_paths(
            PathBuf::from("/usr/bin/false"),
            PathBuf::from("/usr/bin/false"),
        )
    }

    #[test]
    fn small_accepted_image_is_copied_without_touching_the_original() {
        let root = temp_root("small");
        let source = root.join("small.png");
        let original = b"\x89PNG\r\n\x1a\nfixture";
        fs::write(&source, original).unwrap();
        let out = prepare_thumbnail(&source, &unavailable_ffmpeg(), &root.join("out")).unwrap();
        assert_eq!(fs::read(out).unwrap(), original);
        assert_eq!(fs::read(source).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_images_report_conversion_failure_instead_of_size_failure() {
        let root = temp_root("invalid");
        let source = root.join("broken.png");
        fs::write(&source, b"not an image").unwrap();
        let error =
            prepare_thumbnail(&source, &unavailable_ffmpeg(), &root.join("out")).unwrap_err();
        assert_eq!(error.code, "THUMBNAIL_CONVERT_FAILED");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_conversion_scales_large_png_without_an_ffmpeg_png_decoder() {
        let root = temp_root("large-png");
        // An uncompressed random BMP produces a genuinely large PNG, rather
        // than adding padding bytes to an otherwise tiny compressed fixture.
        let (width, height) = (2048u32, 2048u32);
        let pixel_bytes = width * height * 3;
        let mut bitmap = vec![0u8; (54 + pixel_bytes) as usize];
        bitmap[..2].copy_from_slice(b"BM");
        bitmap[2..6].copy_from_slice(&(54 + pixel_bytes).to_le_bytes());
        bitmap[10..14].copy_from_slice(&54u32.to_le_bytes());
        bitmap[14..18].copy_from_slice(&40u32.to_le_bytes());
        bitmap[18..22].copy_from_slice(&width.to_le_bytes());
        bitmap[22..26].copy_from_slice(&height.to_le_bytes());
        bitmap[26..28].copy_from_slice(&1u16.to_le_bytes());
        bitmap[28..30].copy_from_slice(&24u16.to_le_bytes());
        let mut random = 0x12345678u32;
        for byte in &mut bitmap[54..] {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            *byte = random as u8;
        }
        let bmp = root.join("noise.bmp");
        let png = root.join("noise.png");
        fs::write(&bmp, bitmap).unwrap();
        assert!(Command::new("/usr/bin/sips")
            .args(["-s", "format", "png"])
            .arg(&bmp)
            .arg("--out")
            .arg(&png)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
        let original = fs::read(&png).unwrap();
        assert!(original.len() as u64 > MAX_THUMBNAIL_BYTES);
        let output = prepare_thumbnail(&png, &unavailable_ffmpeg(), &root.join("out")).unwrap();
        assert!(fs::metadata(&output).unwrap().len() <= TARGET_THUMBNAIL_BYTES);
        assert!(is_jpeg(&image_header(&output).unwrap()));
        let dimensions = Command::new("/usr/bin/sips")
            .args(["-g", "pixelWidth", "-g", "pixelHeight"])
            .arg(&output)
            .output()
            .unwrap();
        let dimensions = String::from_utf8_lossy(&dimensions.stdout);
        let dimension = |name: &str| {
            dimensions
                .lines()
                .find_map(|line| {
                    let (key, value) = line.trim().split_once(':')?;
                    (key == name).then(|| value.trim().parse::<u32>().unwrap())
                })
                .unwrap()
        };
        let width = dimension("pixelWidth");
        let height = dimension("pixelHeight");
        assert!((640..=1920).contains(&width));
        assert_eq!(width, height, "keep the square image's aspect ratio");
        assert_eq!(fs::read(&png).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires explicitly supplied local thumbnail paths"]
    fn supplied_failed_covers_convert_below_upload_limit() {
        let sources: Vec<PathBuf> =
            serde_json::from_str(&std::env::var("HONGGUO_TEST_THUMBNAIL_FILES").unwrap()).unwrap();
        assert!(!sources.is_empty());
        let root = temp_root("user-covers");
        for (index, source) in sources.iter().enumerate() {
            let original = fs::read(source).unwrap();
            let output =
                prepare_thumbnail(source, &unavailable_ffmpeg(), &root.join(index.to_string()))
                    .unwrap();
            let size = fs::metadata(&output).unwrap().len();
            assert!(size <= TARGET_THUMBNAIL_BYTES);
            assert!(is_jpeg(&image_header(&output).unwrap()));
            assert_eq!(fs::read(source).unwrap(), original);
            println!("cover {}: {} -> {} bytes", index + 1, original.len(), size);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reference_urls_allow_only_https_youtube_cdn_without_credentials_or_custom_ports() {
        for url in [
            "https://i.ytimg.com/vi/video/mqdefault.jpg",
            "https://i9.ytimg.com/vi/video/mqdefault.jpg?sqp=signed-value",
            "https://img.youtube.com/vi/video/0.jpg",
        ] {
            assert!(reference_thumbnail_url(url).is_ok(), "{url}");
        }
        for url in [
            "http://i.ytimg.com/vi/video/mqdefault.jpg",
            "https://i.ytimg.com.evil.test/cover.jpg",
            "https://notytimg.com/cover.jpg",
            "https://127.0.0.1/cover.jpg",
            "https://example.com/cover.jpg",
            "https://user:secret@i.ytimg.com/cover.jpg",
            "https://i.ytimg.com:8443/cover.jpg",
            "https://i.ytimg.com/cover.jpg#fragment",
            "file:///tmp/cover.jpg",
        ] {
            assert!(reference_thumbnail_url(url).is_err(), "{url}");
        }
    }

    async fn reference_response(raw: Vec<u8>) -> reqwest::Response {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(&raw);
        });
        Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap()
            .get(format!("http://{address}/thumbnail"))
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn reference_response_accepts_images_and_rejects_html_and_redirects() {
        let image = b"\xff\xd8\xfffixture";
        let mut raw = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
        raw.extend_from_slice(image);
        assert_eq!(
            read_reference_thumbnail(reference_response(raw).await)
                .await
                .unwrap(),
            image
        );
        let html = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n<html>proxy page</html>".to_vec();
        assert_eq!(
            read_reference_thumbnail(reference_response(html).await)
                .await
                .unwrap_err()
                .code,
            "THUMBNAIL_INVALID"
        );
        let redirect = b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/private\r\nContent-Length: 0\r\n\r\n".to_vec();
        assert_eq!(
            read_reference_thumbnail(reference_response(redirect).await)
                .await
                .unwrap_err()
                .code,
            "YOUTUBE_THUMBNAIL_FETCH_FAILED"
        );
    }

    #[tokio::test]
    async fn reference_response_bounds_both_declared_and_streamed_size() {
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            MAX_REFERENCE_THUMBNAIL_BYTES + 1
        )
        .into_bytes();
        assert_eq!(
            read_reference_thumbnail(reference_response(raw).await)
                .await
                .unwrap_err()
                .code,
            "THUMBNAIL_INVALID"
        );
        let mut raw = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:X}\r\n",
            MAX_REFERENCE_THUMBNAIL_BYTES + 1
        )
        .into_bytes();
        raw.resize(raw.len() + MAX_REFERENCE_THUMBNAIL_BYTES + 1, 1);
        raw.extend_from_slice(b"\r\n0\r\n\r\n");
        assert_eq!(
            read_reference_thumbnail(reference_response(raw).await)
                .await
                .unwrap_err()
                .code,
            "THUMBNAIL_INVALID"
        );
    }

    #[tokio::test]
    #[ignore = "requires an explicitly supplied YouTube thumbnail URL and working system network"]
    async fn supplied_youtube_reference_download_uses_system_network() {
        let url = std::env::var("HONGGUO_TEST_YOUTUBE_THUMBNAIL_URL").unwrap();
        let bytes = fetch_reference_thumbnail(&url).await.unwrap();
        assert!(crate::cover_extension(&bytes).is_some());
        assert!(bytes.len() <= MAX_REFERENCE_THUMBNAIL_BYTES);
        println!("YouTube reference downloaded: {} bytes", bytes.len());
    }

    #[test]
    fn accepts_only_jpeg_and_png_signatures() {
        assert!(is_jpeg(&[0xff, 0xd8, 0xff, 0x00]));
        assert!(is_png(b"\x89PNG\r\n\x1a\nrest"));
        assert!(!is_jpeg(b"GIF89a"));
        assert!(!is_png(b"GIF89a"));
    }
}
