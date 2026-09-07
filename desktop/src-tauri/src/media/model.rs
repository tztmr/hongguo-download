use crate::app_error::AppError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const MEDIA_JOBS_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaJobStatus {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaJobPauseOrigin {
    Queued,
    Running,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaJobKind {
    Merge,
    SeparateBackgroundMusic,
    ExtractSubtitles,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaJobScope {
    Episodes,
    Merged,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InputSnapshot {
    pub path: PathBuf,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MergeInput {
    pub episode_index: u32,
    pub path: PathBuf,
    pub size: u64,
    pub modified_unix_nanos: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StartMergeInput {
    pub episode_index: u32,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum MergeConflictPolicy {
    #[default]
    FailIfExists,
    Overwrite,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MergeMode {
    Auto,
    Copy,
    Transcode,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MergeQuality {
    #[default]
    High,
    Balanced,
    Compact,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MergeRequest {
    pub series_root: PathBuf,
    pub output_file_name: String,
    pub inputs: Vec<MergeInput>,
    #[serde(default)]
    pub transcode_h264: bool,
    #[serde(default)]
    pub mode: Option<MergeMode>,
    #[serde(default)]
    pub quality: MergeQuality,
    #[serde(default)]
    pub conflict_policy: MergeConflictPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StartMergeRequest {
    pub book_id: String,
    pub title: String,
    pub series_root: PathBuf,
    pub output_file_name: String,
    pub inputs: Vec<StartMergeInput>,
    #[serde(default)]
    pub transcode_h264: bool,
    #[serde(default)]
    pub mode: Option<MergeMode>,
    #[serde(default)]
    pub quality: MergeQuality,
    #[serde(default)]
    pub conflict_policy: MergeConflictPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StartAIJobRequest {
    pub book_id: String,
    pub title: String,
    pub series_root: PathBuf,
    pub scope: MediaJobScope,
    pub inputs: Vec<StartMergeInput>,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedAIJobRequest {
    pub book_id: String,
    pub title: String,
    pub kind: MediaJobKind,
    pub scope: MediaJobScope,
    pub series_root: PathBuf,
    pub inputs: Vec<MergeInput>,
    pub model: String,
    pub dedupe_key: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaJobOutputKind {
    Vocals,
    BackgroundMusic,
    NoBackgroundMusicVideo,
    Subtitles,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MediaJobOutput {
    pub episode_index: u32,
    pub kind: MediaJobOutputKind,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedMergeRequest {
    pub book_id: String,
    pub title: String,
    pub scope: MediaJobScope,
    pub series_root: PathBuf,
    pub output_file_name: String,
    pub inputs: Vec<MergeInput>,
    #[serde(default)]
    pub transcode_h264: bool,
    #[serde(default)]
    pub mode: Option<MergeMode>,
    #[serde(default)]
    pub quality: MergeQuality,
    pub conflict_policy: MergeConflictPolicy,
    pub dedupe_key: String,
}

impl ValidatedMergeRequest {
    pub fn execution_request(&self) -> MergeRequest {
        MergeRequest {
            series_root: self.series_root.clone(),
            output_file_name: self.output_file_name.clone(),
            inputs: self.inputs.clone(),
            transcode_h264: self.transcode_h264,
            mode: self.mode,
            quality: self.quality,
            conflict_policy: self.conflict_policy,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaJob {
    pub id: String,
    pub dedupe_key: String,
    pub kind: MediaJobKind,
    pub status: MediaJobStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_origin: Option<MediaJobPauseOrigin>,
    pub stage: String,
    pub percent: f64,
    pub inputs: Vec<InputSnapshot>,
    pub output_path: Option<PathBuf>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    #[serde(default)]
    pub merge_request: Option<ValidatedMergeRequest>,
    #[serde(default)]
    pub ai_request: Option<ValidatedAIJobRequest>,
    #[serde(default)]
    pub outputs: Vec<MediaJobOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_notified_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_notified_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaJobRequest {
    pub dedupe_key: String,
    pub kind: MediaJobKind,
    pub inputs: Vec<InputSnapshot>,
}

#[derive(Debug, Clone)]
pub enum MediaJobTransition {
    Start,
    PauseQueued,
    PauseRunning,
    ResumeQueued,
    ResumeRunning,
    Complete {
        output_path: PathBuf,
        outputs: Vec<MediaJobOutput>,
    },
    Fail {
        code: String,
        message: String,
    },
    Cancel,
    Retry,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaJobsSnapshot {
    pub version: u32,
    pub jobs: Vec<MediaJob>,
    pub warning: Option<AppError>,
}

impl MediaJobsSnapshot {
    pub(crate) fn empty(warning: Option<AppError>) -> Self {
        Self {
            version: MEDIA_JOBS_VERSION,
            jobs: Vec::new(),
            warning,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersistedMediaJobs {
    pub version: u32,
    pub jobs: Vec<MediaJob>,
}

#[cfg(test)]
mod ai_request_tests {
    use super::*;

    #[test]
    fn ai_request_round_trip_keeps_scope_model_and_operation_separate() {
        let request = ValidatedAIJobRequest {
            book_id: "book-1".into(),
            title: "测试剧".into(),
            kind: MediaJobKind::SeparateBackgroundMusic,
            scope: MediaJobScope::Episodes,
            series_root: PathBuf::from("/tmp/series"),
            inputs: vec![MergeInput {
                episode_index: 1,
                path: PathBuf::from("/tmp/series/1.mp4"),
                size: 10,
                modified_unix_nanos: 20,
            }],
            model: "htdemucs".into(),
            dedupe_key: "ai-key".into(),
        };

        let encoded = serde_json::to_vec(&request).unwrap();
        let decoded: ValidatedAIJobRequest = serde_json::from_slice(&encoded).unwrap();

        assert_eq!(decoded, request);
        assert_eq!(decoded.scope, MediaJobScope::Episodes);
        assert_eq!(decoded.kind, MediaJobKind::SeparateBackgroundMusic);
    }
}
