use crate::AppError;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PrivacyStatus {
    Private,
    Unlisted,
    Public,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadIntent {
    pub job_id: String,
    pub file_path: PathBuf,
    pub cover_path: Option<PathBuf>,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub category_id: String,
    pub privacy_status: PrivacyStatus,
    pub self_declared_made_for_kids: bool,
    pub contains_synthetic_media: bool,
    pub audience_confirmed: bool,
    pub synthetic_media_confirmed: bool,
    pub publish_confirmed: bool,
}

impl UploadIntent {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.job_id.is_empty()
            || self.job_id.len() > 128
            || !self
                .job_id
                .chars()
                .all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_'))
        {
            return Err(AppError::new(
                "UPLOAD_REQUEST_INVALID",
                "YouTube 上传请求无效",
            ));
        }
        let title_chars = self.title.trim().chars().count();
        if title_chars == 0 || title_chars > 100 || self.description.chars().count() > 5_000 {
            return Err(AppError::new(
                "UPLOAD_METADATA_INVALID",
                "YouTube 标题或简介无效",
            ));
        }
        if self.category_id.is_empty()
            || !self.category_id.chars().all(|value| value.is_ascii_digit())
            || self.tags.iter().any(|tag| tag.trim().is_empty())
            || self.tags.join(",").chars().count() > 500
        {
            return Err(AppError::new(
                "UPLOAD_METADATA_INVALID",
                "YouTube 分类或标签无效",
            ));
        }
        validate_regular_file(&self.file_path, "UPLOAD_SOURCE_INVALID", "上传源文件无效")?;
        if let Some(cover) = &self.cover_path {
            validate_regular_file(cover, "UPLOAD_COVER_INVALID", "上传封面无效")?;
        }
        if !self.audience_confirmed {
            return Err(AppError::new(
                "UPLOAD_AUDIENCE_CONFIRMATION_REQUIRED",
                "请确认视频的儿童受众设置",
            ));
        }
        if !self.synthetic_media_confirmed {
            return Err(AppError::new(
                "UPLOAD_SYNTHETIC_CONFIRMATION_REQUIRED",
                "请确认是否包含合成内容",
            ));
        }
        if !self.publish_confirmed {
            return Err(AppError::new(
                "UPLOAD_CONFIRMATION_REQUIRED",
                "请明确确认发布 YouTube 视频",
            ));
        }
        Ok(())
    }
}

fn validate_regular_file(path: &PathBuf, code: &str, message: &str) -> Result<(), AppError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| AppError::new(code, message))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() == 0 {
        return Err(AppError::new(code, message));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CredentialSummary {
    pub configured: bool,
    pub client_id_suffix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub channel_id: String,
    pub title: String,
    pub authorized_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum YouTubeJobStatus {
    Queued,
    PreparingAuthorization,
    CreatingSession,
    Uploading,
    WaitingToRetry,
    Processing,
    SettingThumbnail,
    Completed,
    VideoUploadedThumbnailFailed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ThumbnailState {
    Pending,
    Succeeded,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeJob {
    pub id: String,
    pub title: String,
    pub channel_id: String,
    pub source_path: PathBuf,
    pub status: YouTubeJobStatus,
    pub uploaded_bytes: u64,
    pub total_bytes: u64,
    pub percent: f64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub video_id: Option<String>,
    pub youtube_url: Option<String>,
    pub actual_privacy_status: Option<PrivacyStatus>,
    pub thumbnail_state: ThumbnailState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_notified_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_notified_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadResult {
    pub video_id: String,
    pub youtube_url: String,
    pub privacy_status: Option<PrivacyStatus>,
    pub thumbnail_state: ThumbnailState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeSnapshot {
    pub credential: CredentialSummary,
    pub channels: Vec<AccountSummary>,
    pub active_channel_id: Option<String>,
    pub jobs: Vec<YouTubeJob>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn upload_intent_requires_audience_synthetic_and_publish_confirmation() {
        let root =
            std::env::temp_dir().join(format!("hongguo-upload-intent-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("merged.mp4");
        fs::write(&path, b"video").unwrap();
        let mut intent = UploadIntent {
            job_id: "job-1".into(),
            file_path: path,
            cover_path: None,
            title: "测试剧".into(),
            description: "测试剧".into(),
            tags: vec!["短剧".into()],
            category_id: "24".into(),
            privacy_status: PrivacyStatus::Public,
            self_declared_made_for_kids: false,
            contains_synthetic_media: true,
            audience_confirmed: false,
            synthetic_media_confirmed: true,
            publish_confirmed: true,
        };
        assert_eq!(
            intent.validate().unwrap_err().code,
            "UPLOAD_AUDIENCE_CONFIRMATION_REQUIRED"
        );
        intent.audience_confirmed = true;
        intent.synthetic_media_confirmed = false;
        assert_eq!(
            intent.validate().unwrap_err().code,
            "UPLOAD_SYNTHETIC_CONFIRMATION_REQUIRED"
        );
        intent.synthetic_media_confirmed = true;
        intent.publish_confirmed = false;
        assert_eq!(
            intent.validate().unwrap_err().code,
            "UPLOAD_CONFIRMATION_REQUIRED"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn public_snapshot_never_serializes_token_or_session_fields() {
        let snapshot = YouTubeSnapshot {
            credential: CredentialSummary {
                configured: true,
                client_id_suffix: "…123456".into(),
            },
            channels: vec![AccountSummary {
                channel_id: "UC_SAFE".into(),
                title: "测试频道".into(),
                authorized_at: "2026-09-03T00:00:00Z".into(),
            }],
            active_channel_id: Some("UC_SAFE".into()),
            jobs: Vec::new(),
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("refresh"));
        assert!(!json.contains("Bearer"));
        assert!(!json.contains("upload_session"));
    }
}
