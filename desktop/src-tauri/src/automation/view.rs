//! Keep polling payloads independent of episode counts and saved AI responses.
//! The durable Snapshot remains the sole source of recovery/cleanup evidence.
use super::model::*;
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotView {
    config: Option<Value>,
    mode: Mode,
    jobs: Vec<TaskView>,
    waiting: Vec<TaskView>,
    logs: Vec<Log>,
    last_scan: u64,
    next_scan: u64,
    warning: String,
    key_status: KeyStatus,
    scan_summary: ScanSummary,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskView {
    id: String,
    title: String,
    book_id: String,
    season: Option<u32>,
    queue_order: u64,
    stage: String,
    status: Status,
    media_state: Option<String>,
    message: String,
    episode_done: usize,
    episode_total: usize,
    progress: f64,
    main_video_url: String,
    short_video_url: String,
    updated_at: u64,
    config: Value,
    attempts: u64,
    retry_at: u64,
}

impl From<&Task> for TaskView {
    fn from(job: &Task) -> Self {
        Self {
            id: job.id.clone(),
            title: job.title.clone(),
            book_id: job.book_id.clone(),
            season: job.season,
            queue_order: job.queue_order,
            stage: job.stage.clone(),
            status: job.status.clone(),
            media_state: job.media_state.clone(),
            message: job.message.clone(),
            episode_done: job.episode_done,
            episode_total: job.episode_total,
            progress: job.progress,
            main_video_url: job.main_video_url.clone(),
            short_video_url: job.short_video_url.clone(),
            updated_at: job.updated_at,
            config: serde_json::json!({"channel": text(&job.config, "channel")}),
            attempts: job.attempts,
            retry_at: job.retry_at,
        }
    }
}

impl From<&Snapshot> for SnapshotView {
    fn from(snapshot: &Snapshot) -> Self {
        Self {
            config: snapshot.config.clone(),
            mode: snapshot.mode.clone(),
            jobs: snapshot.jobs.iter().map(TaskView::from).collect(),
            waiting: snapshot.waiting.iter().map(TaskView::from).collect(),
            logs: snapshot.logs.clone(),
            last_scan: snapshot.last_scan,
            next_scan: snapshot.next_scan,
            warning: snapshot.warning.clone(),
            key_status: snapshot.key_status.clone(),
            scan_summary: snapshot.scan_summary.clone(),
        }
    }
}
