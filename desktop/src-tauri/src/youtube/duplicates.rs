use super::{config::SecretString, format::UploadFormat};
use crate::AppError;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadIdentity {
    pub channel_id: String,
    pub book_id: String,
    pub drama_title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default)]
    pub allow_duplicate: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateQuery {
    pub channel_id: String,
    pub title: String,
    pub book_id: String,
    pub drama_title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "UploadFormat::is_auto")]
    pub upload_format: UploadFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownVideo {
    pub channel_id: String,
    pub video_id: String,
    pub title: String,
    #[serde(default)]
    pub book_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub drama_title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "UploadFormat::is_auto")]
    pub upload_format: UploadFormat,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateMatch {
    pub title: String,
    pub video_id: String,
    pub youtube_url: String,
    pub reason: String,
    pub confidence: MatchConfidence,
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

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MatchConfidence {
    Confirmed,
    Possible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Season {
    Missing,
    Known(u32),
    Ambiguous,
}

fn season_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r"(?i)第\s*([0-9零〇一二两兩三四五六七八九十百千]+)\s*季|(?:season|s)\s*([0-9]{1,3})",
        )
        .unwrap()
    })
}

fn season_number(text: &str) -> Option<u32> {
    if let Ok(n) = text.parse::<u32>() {
        return (n > 0 && n <= 999).then_some(n);
    }
    let mut total = 0;
    let mut digit = 0;
    for c in text.chars() {
        match c {
            '零' | '〇' => digit = 0,
            '一' => digit = 1,
            '二' | '两' | '兩' => digit = 2,
            '三' => digit = 3,
            '四' => digit = 4,
            '五' => digit = 5,
            '六' => digit = 6,
            '七' => digit = 7,
            '八' => digit = 8,
            '九' => digit = 9,
            '十' => {
                total += digit.max(1) * 10;
                digit = 0;
            }
            '百' => {
                total += digit.max(1) * 100;
                digit = 0;
            }
            _ => return None,
        }
    }
    let n = total + digit;
    (n > 0 && n <= 999).then_some(n)
}

fn detected_season(title: &str) -> Season {
    static RANGE: OnceLock<Regex> = OnceLock::new();
    if RANGE.get_or_init(|| Regex::new(r"第\s*[0-9一二三四五六七八九十]+\s*[-–—~～、至到]\s*[0-9一二三四五六七八九十]+\s*季").unwrap()).is_match(title) { return Season::Ambiguous; }
    let mut seasons = HashSet::new();
    for capture in season_pattern().captures_iter(title) {
        let matched = capture.get(0).unwrap();
        if capture.get(2).is_some()
            && (title[..matched.start()]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric())
                || title[matched.end()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit()))
        {
            continue;
        }
        let number = capture.get(1).or_else(|| capture.get(2)).unwrap().as_str();
        match season_number(number) {
            Some(n) => {
                seasons.insert(n);
            }
            None => return Season::Ambiguous,
        }
    }
    match seasons.len() {
        0 => Season::Missing,
        1 => Season::Known(*seasons.iter().next().unwrap()),
        _ => Season::Ambiguous,
    }
}

fn effective_season(explicit: Option<u32>, drama: &str, title: &str) -> Season {
    if let Some(n) = explicit {
        return if n > 0 && n <= 999 {
            Season::Known(n)
        } else {
            Season::Ambiguous
        };
    }
    match detected_season(drama) {
        Season::Missing => detected_season(title),
        season => season,
    }
}

pub fn inferred_season(explicit: Option<u32>, drama: &str, title: &str) -> Option<u32> {
    match effective_season(explicit, drama, title) {
        Season::Known(n) => Some(n),
        _ => None,
    }
}

fn title_root(title: &str) -> String {
    compact_title(&season_pattern().replace_all(title, ""))
}

pub fn match_decision(
    query: &DuplicateQuery,
    video: &KnownVideo,
) -> Option<(&'static str, MatchConfidence)> {
    if query.channel_id != video.channel_id {
        return None;
    }
    let season = effective_season(query.season, &query.drama_title, &query.title);
    let existing_season = effective_season(video.season, &video.drama_title, &video.title);
    if matches!((season, existing_season), (Season::Known(a), Season::Known(b)) if a != b) {
        return None;
    }
    if query.upload_format != UploadFormat::Auto
        && video.upload_format != UploadFormat::Auto
        && query.upload_format != video.upload_format
    {
        return None;
    }
    let same_book = !query.book_id.is_empty() && query.book_id == video.book_id;
    if same_book {
        let same_season = season == existing_season && season != Season::Ambiguous;
        let same_format =
            query.upload_format != UploadFormat::Auto && query.upload_format == video.upload_format;
        return Some(if same_season && same_format {
            ("sameDrama", MatchConfidence::Confirmed)
        } else {
            ("identityIncomplete", MatchConfidence::Possible)
        });
    }
    let title = normalized_title(&query.title);
    if !title.is_empty() && title == normalized_title(&video.title) {
        return Some(("sameTitle", MatchConfidence::Possible));
    }
    let drama = title_root(&query.drama_title);
    let existing = title_root(&video.title);
    let saved = title_root(&video.drama_title);
    if drama.chars().count() >= 2
        && (existing.contains(&drama) || (!saved.is_empty() && saved == drama))
    {
        return Some(("similarTitle", MatchConfidence::Possible));
    }
    None
}

pub fn match_reason(query: &DuplicateQuery, video: &KnownVideo) -> Option<&'static str> {
    match_decision(query, video).map(|(reason, _)| reason)
}

pub fn find_matches(query: &DuplicateQuery, videos: &[KnownVideo]) -> Vec<DuplicateMatch> {
    // Remote titles are current, but local records carry source identity. Merge before matching
    // so a metadata-poor remote row cannot mask a known different season or video format.
    let mut merged: Vec<KnownVideo> = Vec::new();
    let mut indices = HashMap::new();
    for video in videos {
        let key = (video.channel_id.clone(), video.video_id.clone());
        if let Some(&index) = indices.get(&key) {
            let current: &mut KnownVideo = &mut merged[index];
            if current.book_id.is_empty() {
                current.book_id = video.book_id.clone();
            }
            if current.drama_title.is_empty() {
                current.drama_title = video.drama_title.clone();
            }
            if current.season.is_none() {
                current.season = video.season;
            }
            if current.upload_format == UploadFormat::Auto {
                current.upload_format = video.upload_format;
            }
        } else {
            indices.insert(key, merged.len());
            merged.push(video.clone());
        }
    }
    merged
        .into_iter()
        .filter_map(|video| {
            let (reason, confidence) = match_decision(query, &video)?;
            let youtube_url = if video.video_id.is_empty() || video.video_id.starts_with("queued:")
            {
                String::new()
            } else {
                format!("https://www.youtube.com/watch?v={}", video.video_id)
            };
            Some(DuplicateMatch {
                title: video.title,
                video_id: video.video_id,
                youtube_url,
                reason: reason.into(),
                confidence,
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
                ..Default::default()
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
            upload_format: UploadFormat::Standard,
            ..Default::default()
        }
    }
    fn video(id: &str, title: &str, book: &str) -> KnownVideo {
        KnownVideo {
            channel_id: "channel-a".into(),
            video_id: id.into(),
            title: title.into(),
            book_id: book.into(),
            upload_format: UploadFormat::Standard,
            ..Default::default()
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
    #[test]
    fn separates_seasons_even_when_source_reuses_id_and_ai_changes_title() {
        let mut q = query();
        let mut existing = video("old", "AI 改写后的标题", "book-1");
        existing.drama_title = "AI 漫剧 第一季".into();
        for name in [
            "AI 漫剧 第二季",
            "AI 漫剧 第三季",
            "AI 漫剧 Season 2",
            "AI 漫剧 S03",
        ] {
            q.drama_title = name.into();
            assert_eq!(match_decision(&q, &existing), None, "{name}");
        }
    }

    #[test]
    fn recognizes_equivalent_season_spellings_and_explicit_override() {
        let mut q = query();
        q.season = Some(2);
        for name in [
            "AI 漫剧 第二季",
            "AI 漫剧 第2季",
            "AI 漫剧 Season 2",
            "AI 漫剧 S02",
        ] {
            let existing = video("old", name, "book-1");
            assert_eq!(
                match_decision(&q, &existing),
                Some(("sameDrama", MatchConfidence::Confirmed))
            );
        }
        q.drama_title = "AI 漫剧 第一季".into();
        assert_eq!(inferred_season(q.season, &q.drama_title, &q.title), Some(2));
        assert_eq!(
            inferred_season(None, "AI 漫剧 第二季", "AI 标题第三季"),
            Some(2)
        );
        assert_eq!(inferred_season(None, "AI 漫剧", "剧情钩子 S03"), Some(3));
    }

    #[test]
    fn incomplete_or_ambiguous_season_requires_review_not_automatic_skip() {
        let mut q = query();
        q.season = Some(2);
        for name in [
            "AI 漫剧",
            "AI 漫剧 第1-3季",
            "AI 漫剧 第一季 第二季",
            "AI 漫剧 S01-S03",
        ] {
            assert_eq!(
                match_decision(&q, &video("old", name, "book-1")),
                Some(("identityIncomplete", MatchConfidence::Possible)),
                "{name}"
            );
        }
        assert_eq!(inferred_season(None, "AI 漫剧", ""), None);
        assert_eq!(inferred_season(None, "News2026", ""), None);
        assert_eq!(
            match_decision(&query(), &video("old", "AI 漫剧", "")),
            Some(("sameTitle", MatchConfidence::Possible))
        );
    }

    #[test]
    fn separates_shorts_from_standard_and_reviews_legacy_unknown_format() {
        let mut existing = video("old", "AI 漫剧", "book-1");
        existing.upload_format = UploadFormat::Shorts;
        assert_eq!(match_decision(&query(), &existing), None);
        let legacy: KnownVideo = serde_json::from_value(serde_json::json!({"channelId":"channel-a", "videoId":"old", "title":"AI 漫剧", "bookId":"book-1"})).unwrap();
        assert_eq!(legacy.upload_format, UploadFormat::Auto);
        assert_eq!(
            match_decision(&query(), &legacy),
            Some(("identityIncomplete", MatchConfidence::Possible))
        );
    }

    #[test]
    fn merges_remote_and_local_identity_before_matching() {
        let mut remote = video("same-video", "AI 漫剧", "");
        remote.upload_format = UploadFormat::Auto;
        let mut local = video("same-video", "旧标题", "book-1");
        local.season = Some(1);
        let mut q = query();
        q.season = Some(2);
        assert!(find_matches(&q, &[remote.clone(), local.clone()]).is_empty());
        local.season = Some(2);
        local.upload_format = UploadFormat::Shorts;
        assert!(find_matches(&q, &[remote.clone(), local.clone()]).is_empty());
        local.upload_format = UploadFormat::Standard;
        let result = find_matches(&q, &[remote, local]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].confidence, MatchConfidence::Confirmed);
        assert_eq!(result[0].title, "AI 漫剧");
    }

    #[test]
    fn queued_matches_do_not_link_to_nonexistent_youtube_videos() {
        let result = find_matches(&query(), &[video("queued:job-1", "AI 漫剧", "book-1")]);
        assert_eq!(result.len(), 1);
        assert!(result[0].youtube_url.is_empty());
    }

    #[test]
    fn legacy_identity_roundtrip_keeps_checkpoint_serialization_unchanged() {
        let old = serde_json::json!({"channelId":"c", "bookId":"b", "dramaTitle":"剧名", "allowDuplicate":false});
        let identity: UploadIdentity = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(identity.season, None);
        assert_eq!(serde_json::to_value(identity).unwrap(), old);
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
