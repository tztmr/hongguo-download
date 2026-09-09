use super::config::SecretString;
use crate::AppError;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, io::Write, path::Path, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadIdentity {
    pub channel_id: String,
    pub book_id: String,
    pub drama_title: String,
    #[serde(default)]
    pub allow_duplicate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateQuery {
    pub channel_id: String,
    pub title: String,
    pub book_id: String,
    pub drama_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownVideo {
    pub channel_id: String,
    pub video_id: String,
    pub title: String,
    #[serde(default)]
    pub book_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateMatch {
    pub title: String,
    pub video_id: String,
    pub youtube_url: String,
    pub reason: String,
}

fn normalized_title(title: &str) -> String {
    title.trim().to_lowercase()
}

fn compact_title(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn match_reason(query: &DuplicateQuery, video: &KnownVideo) -> Option<&'static str> {
    if query.channel_id != video.channel_id {
        return None;
    }
    if !query.book_id.is_empty() && query.book_id == video.book_id {
        return Some("sameDrama");
    }
    let title = normalized_title(&query.title);
    if !title.is_empty() && title == normalized_title(&video.title) {
        return Some("sameTitle");
    }
    let drama = compact_title(&query.drama_title);
    let existing = compact_title(&video.title);
    if drama.chars().count() >= 2 && existing.contains(&drama) {
        return Some("similarTitle");
    }
    None
}

pub fn find_matches(query: &DuplicateQuery, videos: &[KnownVideo]) -> Vec<DuplicateMatch> {
    let mut seen = HashSet::new();
    videos
        .iter()
        .filter_map(|video| {
            let reason = match_reason(query, video)?;
            if !seen.insert(video.video_id.clone()) {
                return None;
            }
            Some(DuplicateMatch {
                title: video.title.clone(),
                video_id: video.video_id.clone(),
                youtube_url: format!("https://www.youtube.com/watch?v={}", video.video_id),
                reason: reason.into(),
            })
        })
        .collect()
}

fn lookup_error() -> AppError {
    AppError::new(
        "YOUTUBE_DUPLICATE_CHECK_FAILED",
        "无法完整读取频道影片，尚未完成查重，请检查网络后重试",
    )
}

#[derive(Deserialize)]
struct ChannelList {
    items: Vec<Channel>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Channel {
    id: String,
    content_details: ChannelDetails,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelDetails {
    related_playlists: RelatedPlaylists,
}
#[derive(Deserialize)]
struct RelatedPlaylists {
    uploads: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaylistPage {
    items: Vec<PlaylistVideo>,
    next_page_token: Option<String>,
}
#[derive(Deserialize)]
struct PlaylistVideo {
    snippet: VideoSnippet,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoSnippet {
    title: String,
    resource_id: VideoResource,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoResource {
    video_id: String,
}

async fn get_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    params: &[(&str, &str)],
    token: &SecretString,
) -> Result<T, AppError> {
    let response = client
        .get(url)
        .query(params)
        .bearer_auth(token.expose_secret())
        .send()
        .await
        .map_err(|_| lookup_error())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(AppError::new(
            "AUTH_REQUIRED",
            "频道授权已失效，请重新授权后查重",
        ));
    }
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        return Err(AppError::new(
            "YOUTUBE_DUPLICATE_CHECK_FORBIDDEN",
            "无法读取频道影片，请检查 YouTube API 配额及账号读取权限，必要时重新授权",
        ));
    }
    if !response.status().is_success() {
        return Err(lookup_error());
    }
    response.json().await.map_err(|_| lookup_error())
}

pub async fn channel_videos(
    channel_id: &str,
    token: &SecretString,
) -> Result<Vec<KnownVideo>, AppError> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| lookup_error())?;
    fetch_channel_videos(
        &client,
        "https://www.googleapis.com/youtube/v3",
        channel_id,
        token,
    )
    .await
}

async fn fetch_channel_videos(
    client: &reqwest::Client,
    base: &str,
    channel_id: &str,
    token: &SecretString,
) -> Result<Vec<KnownVideo>, AppError> {
    let channels: ChannelList = get_json(
        client,
        &format!("{base}/channels"),
        &[("part", "contentDetails"), ("mine", "true")],
        token,
    )
    .await?;
    let channel = channels
        .items
        .into_iter()
        .find(|c| c.id == channel_id)
        .ok_or_else(|| {
            AppError::new(
                "YOUTUBE_CHANNEL_MISMATCH",
                "授权频道与所选频道不一致，请重新授权",
            )
        })?;
    let playlist = channel.content_details.related_playlists.uploads;
    if playlist.is_empty() {
        return Err(lookup_error());
    }
    let mut videos = Vec::new();
    let mut page_token = String::new();
    let mut seen = HashSet::new();
    loop {
        let mut params = vec![
            ("part", "snippet"),
            ("playlistId", playlist.as_str()),
            ("maxResults", "50"),
        ];
        if !page_token.is_empty() {
            params.push(("pageToken", page_token.as_str()));
        }
        let page: PlaylistPage =
            get_json(client, &format!("{base}/playlistItems"), &params, token).await?;
        for item in page.items {
            let id = item.snippet.resource_id.video_id;
            if id.is_empty()
                || !id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(lookup_error());
            }
            videos.push(KnownVideo {
                channel_id: channel_id.into(),
                video_id: id,
                title: item.snippet.title,
                book_id: String::new(),
            });
        }
        match page.next_page_token.filter(|t| !t.is_empty()) {
            Some(next) if seen.insert(next.clone()) => page_token = next,
            Some(_) => return Err(lookup_error()),
            None => return Ok(videos),
        }
    }
}

fn history_error() -> AppError {
    AppError::new(
        "YOUTUBE_HISTORY_FAILED",
        "无法读写上传去重记录，请检查数据目录后重试",
    )
}

pub fn read_history(data_dir: &Path) -> Result<Vec<KnownVideo>, AppError> {
    match std::fs::read(data_dir.join("youtube/upload-history.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| history_error()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err(history_error()),
    }
}

// Callers serialize reads and writes with the service history lock.
pub fn remember_video(data_dir: &Path, video: KnownVideo) -> Result<(), AppError> {
    let mut history = read_history(data_dir)?;
    history.retain(|old| old.channel_id != video.channel_id || old.video_id != video.video_id);
    history.push(video);
    let dir = data_dir.join("youtube");
    std::fs::create_dir_all(&dir).map_err(|_| history_error())?;
    let bytes = serde_json::to_vec(&history).map_err(|_| history_error())?;
    atomicwrites::AtomicFile::new(
        dir.join("upload-history.json"),
        atomicwrites::OverwriteBehavior::AllowOverwrite,
    )
    .write(|file| file.write_all(&bytes))
    .map_err(|_| history_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn query() -> DuplicateQuery {
        DuplicateQuery {
            channel_id: "channel-a".into(),
            title: "  AI 漫剧  ".into(),
            book_id: "book-1".into(),
            drama_title: "AI 漫剧".into(),
        }
    }
    fn video(id: &str, title: &str, book: &str) -> KnownVideo {
        KnownVideo {
            channel_id: "channel-a".into(),
            video_id: id.into(),
            title: title.into(),
            book_id: book.into(),
        }
    }
    #[test]
    fn matches_titles_and_saved_identity_only_in_the_selected_channel() {
        let mut other = video("other", "AI 漫剧", "book-1");
        other.channel_id = "channel-b".into();
        let matches = find_matches(
            &query(),
            &[
                video("exact", "ai 漫剧", ""),
                video("renamed", "完全改名", "book-1"),
                video("similar", "《AI 漫剧》全集", ""),
                video("unrelated", "都市爱情", ""),
                other,
                video("exact", "ai 漫剧", ""),
            ],
        );
        assert_eq!(
            matches
                .iter()
                .map(|m| (m.video_id.as_str(), m.reason.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("exact", "sameTitle"),
                ("renamed", "sameDrama"),
                ("similar", "similarTitle")
            ]
        );
        assert_eq!(
            matches[0].youtube_url,
            "https://www.youtube.com/watch?v=exact"
        );
    }
    #[test]
    fn empty_or_short_drama_names_do_not_match_every_video() {
        let mut q = query();
        q.drama_title = "".into();
        q.book_id = "".into();
        q.title = "新剧".into();
        assert!(find_matches(&q, &[video("1", "另一部", "")]).is_empty());
        q.drama_title = "爱".into();
        assert!(find_matches(&q, &[video("1", "爱情故事", "")]).is_empty());
    }
    async fn mock_api(
        replies: Vec<(&'static str, &'static str)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (status, body) in replies {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buf = [0; 4096];
                    let n = stream.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buf[..n]);
                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                requests.push(String::from_utf8(request).unwrap());
                let response = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len());
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (base, task)
    }
    const CHANNEL: &str = r#"{"items":[{"id":"channel-a","contentDetails":{"relatedPlaylists":{"uploads":"uploads-a"}}}]}"#;

    #[tokio::test]
    async fn reads_every_upload_page_and_authenticates_owner_requests() {
        let (base, server) = mock_api(vec![
            ("200 OK", CHANNEL),
            ("200 OK", r#"{"items":[{"snippet":{"title":"其他影片","resourceId":{"videoId":"first"}}}],"nextPageToken":"next-page"}"#),
            ("200 OK", r#"{"items":[{"snippet":{"title":"AI 漫剧","resourceId":{"videoId":"second"}}}]}"#),
        ]).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let videos = fetch_channel_videos(
            &client,
            &base,
            "channel-a",
            &SecretString::new("test-token"),
        )
        .await
        .unwrap();
        assert_eq!(find_matches(&query(), &videos)[0].video_id, "second");
        let requests = server.await.unwrap();
        assert!(requests[0].contains("mine=true"));
        assert!(requests[1].contains("playlistId=uploads-a"));
        assert!(requests[1].contains("maxResults=50"));
        assert!(requests[2].contains("pageToken=next-page"));
        assert!(requests.iter().all(|r| r
            .to_lowercase()
            .contains("authorization: bearer test-token")));
    }

    #[tokio::test]
    async fn failed_or_malformed_later_pages_never_report_a_clean_check() {
        for reply in [
            ("503 Service Unavailable", "{}"),
            ("200 OK", "{}"),
            ("200 OK", r#"{"items":[],"nextPageToken":"repeat"}"#),
        ] {
            let (base, server) = mock_api(vec![
                ("200 OK", CHANNEL),
                ("200 OK", r#"{"items":[],"nextPageToken":"repeat"}"#),
                reply,
            ])
            .await;
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            assert!(fetch_channel_videos(
                &client,
                &base,
                "channel-a",
                &SecretString::new("test-token")
            )
            .await
            .is_err());
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn refuses_the_wrong_authorized_channel() {
        let (base, server) = mock_api(vec![("200 OK", CHANNEL)]).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert_eq!(
            fetch_channel_videos(
                &client,
                &base,
                "channel-b",
                &SecretString::new("test-token")
            )
            .await
            .unwrap_err()
            .code,
            "YOUTUBE_CHANNEL_MISMATCH"
        );
        server.await.unwrap();
    }

    #[test]
    fn history_survives_reload_without_the_upload_queue_and_updates_existing_video() {
        let root = std::env::temp_dir().join(format!(
            "youtube-history-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        remember_video(&root, video("saved", "原始标题", "book-1")).unwrap();
        remember_video(&root, video("saved", "新标题", "book-1")).unwrap();
        let history = read_history(&root).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(find_matches(&query(), &history)[0].reason, "sameDrama");
        std::fs::write(root.join("youtube/upload-history.json"), b"corrupt").unwrap();
        assert!(read_history(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
