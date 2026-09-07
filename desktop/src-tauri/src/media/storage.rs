use super::model::{
    MediaJob, MediaJobPauseOrigin, MediaJobRequest, MediaJobStatus, MediaJobTransition,
    MediaJobsSnapshot, PersistedMediaJobs, ValidatedAIJobRequest, ValidatedMergeRequest,
    MEDIA_JOBS_VERSION,
};
use crate::app_error::AppError;
use crate::platform_fs::{replace_file, sync_directory};
use std::{
    fs::{self, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

const STORE_NAME: &str = "media-jobs.json";
const TEMP_STORE_NAME: &str = "media-jobs.json.tmp";
static JOB_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub struct MediaJobManager {
    store_dir: PathBuf,
    state: Mutex<MediaJobsSnapshot>,
}

impl MediaJobManager {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, AppError> {
        let store_dir = path.as_ref().to_path_buf();
        fs::create_dir_all(&store_dir).map_err(storage_error)?;
        let store_path = store_dir.join(STORE_NAME);
        if !store_path.exists() {
            return Ok(Self {
                store_dir,
                state: Mutex::new(MediaJobsSnapshot::empty(None)),
            });
        }

        let bytes = fs::read(&store_path).map_err(storage_error)?;
        let mut persisted = match serde_json::from_slice::<PersistedMediaJobs>(&bytes) {
            Ok(value) if value.version == MEDIA_JOBS_VERSION => value,
            Ok(_) | Err(_) => {
                quarantine_corrupt_store(&store_dir, &store_path)?;
                return Ok(Self {
                    store_dir,
                    state: Mutex::new(MediaJobsSnapshot::empty(Some(AppError::new(
                        "MEDIA_JOBS_CORRUPT_RECOVERED",
                        "媒体任务记录损坏，已隔离并从空记录恢复",
                    )))),
                });
            }
        };

        let recovered_running = persisted.jobs.iter_mut().fold(false, |changed, job| {
            if job.status == MediaJobStatus::Running
                || (job.status == MediaJobStatus::Paused
                    && job.pause_origin != Some(MediaJobPauseOrigin::Queued))
            {
                job.status = MediaJobStatus::Interrupted;
                job.stage = "interrupted".into();
                job.pause_origin = None;
                true
            } else {
                changed
            }
        });
        if recovered_running {
            persist_jobs(&store_dir, &persisted.jobs)?;
        }

        Ok(Self {
            store_dir,
            state: Mutex::new(MediaJobsSnapshot {
                version: persisted.version,
                jobs: persisted.jobs,
                warning: None,
            }),
        })
    }

    pub fn snapshot(&self) -> MediaJobsSnapshot {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn enqueue(&self, request: MediaJobRequest) -> Result<MediaJob, AppError> {
        self.enqueue_parts(request.dedupe_key, request.kind, request.inputs, None, None)
    }

    pub fn enqueue_merge(&self, request: ValidatedMergeRequest) -> Result<MediaJob, AppError> {
        let inputs = request
            .inputs
            .iter()
            .map(|input| super::model::InputSnapshot {
                path: input.path.clone(),
                size_bytes: input.size,
            })
            .collect();
        self.enqueue_parts(
            request.dedupe_key.clone(),
            super::model::MediaJobKind::Merge,
            inputs,
            Some(request),
            None,
        )
    }

    pub fn enqueue_ai(&self, request: ValidatedAIJobRequest) -> Result<MediaJob, AppError> {
        let inputs = request
            .inputs
            .iter()
            .map(|input| super::model::InputSnapshot {
                path: input.path.clone(),
                size_bytes: input.size,
            })
            .collect();
        self.enqueue_parts(
            request.dedupe_key.clone(),
            request.kind,
            inputs,
            None,
            Some(request),
        )
    }

    fn enqueue_parts(
        &self,
        dedupe_key: String,
        kind: super::model::MediaJobKind,
        inputs: Vec<super::model::InputSnapshot>,
        merge_request: Option<ValidatedMergeRequest>,
        ai_request: Option<ValidatedAIJobRequest>,
    ) -> Result<MediaJob, AppError> {
        let mut guard = self.state.lock().map_err(|_| manager_unavailable_error())?;
        if guard.jobs.iter().any(|job| {
            job.dedupe_key == dedupe_key
                && matches!(
                    job.status,
                    MediaJobStatus::Queued | MediaJobStatus::Running | MediaJobStatus::Paused
                )
        }) {
            return Err(AppError::new(
                "MEDIA_JOB_ALREADY_ACTIVE",
                "同一媒体任务已在队列中或正在运行",
            ));
        }

        let job = MediaJob {
            id: next_job_id(),
            dedupe_key,
            kind,
            status: MediaJobStatus::Queued,
            pause_origin: None,
            stage: "queued".into(),
            percent: 0.0,
            inputs,
            output_path: None,
            error_code: None,
            error_message: None,
            merge_request,
            ai_request,
            outputs: Vec::new(),
            completion_notified_at: None,
            failure_notified_at: None,
        };
        let mut next = guard.clone();
        next.jobs.push(job.clone());
        persist_jobs(&self.store_dir, &next.jobs)?;
        *guard = next;
        Ok(job)
    }

    pub fn job(&self, id: &str) -> Result<MediaJob, AppError> {
        self.state
            .lock()
            .map_err(|_| manager_unavailable_error())?
            .jobs
            .iter()
            .find(|job| job.id == id)
            .cloned()
            .ok_or_else(|| AppError::new("MEDIA_JOB_NOT_FOUND", "媒体任务不存在"))
    }

    pub fn mark_notified(&self, id: &str, succeeded: bool) -> Result<MediaJob, AppError> {
        let mut guard = self.state.lock().map_err(|_| manager_unavailable_error())?;
        let mut next = guard.clone();
        let job = next
            .jobs
            .iter_mut()
            .find(|job| job.id == id)
            .ok_or_else(|| AppError::new("MEDIA_JOB_NOT_FOUND", "媒体任务不存在"))?;
        let terminal_matches = if succeeded {
            job.status == MediaJobStatus::Completed
        } else {
            matches!(
                job.status,
                MediaJobStatus::Failed | MediaJobStatus::Interrupted
            )
        };
        if !terminal_matches {
            return Err(AppError::new(
                "MEDIA_JOB_INVALID_TRANSITION",
                "媒体任务尚未结束",
            ));
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        if succeeded {
            job.completion_notified_at.get_or_insert(timestamp);
        } else {
            job.failure_notified_at.get_or_insert(timestamp);
        }
        let claimed = job.clone();
        persist_jobs(&self.store_dir, &next.jobs)?;
        *guard = next;
        Ok(claimed)
    }

    pub fn merge_request(&self, id: &str) -> Result<ValidatedMergeRequest, AppError> {
        self.job(id)?.merge_request.ok_or_else(|| {
            AppError::new(
                "MEDIA_JOB_RETRY_UNAVAILABLE",
                "媒体任务缺少可重试的原始请求",
            )
        })
    }

    pub fn ai_request(&self, id: &str) -> Result<ValidatedAIJobRequest, AppError> {
        self.job(id)?.ai_request.ok_or_else(|| {
            AppError::new(
                "MEDIA_JOB_RETRY_UNAVAILABLE",
                "媒体任务缺少可重试的 AI 请求",
            )
        })
    }

    pub fn claim_oldest_queued(&self) -> Result<Option<MediaJob>, AppError> {
        let mut guard = self.state.lock().map_err(|_| manager_unavailable_error())?;
        let Some(index) = guard
            .jobs
            .iter()
            .position(|job| job.status == MediaJobStatus::Queued)
        else {
            return Ok(None);
        };
        let mut next = guard.clone();
        apply_transition(&mut next.jobs[index], MediaJobTransition::Start)?;
        let claimed = next.jobs[index].clone();
        persist_jobs(&self.store_dir, &next.jobs)?;
        *guard = next;
        Ok(Some(claimed))
    }

    pub fn restore_claimed_to_queued(&self, id: &str) -> Result<MediaJob, AppError> {
        let mut guard = self.state.lock().map_err(|_| manager_unavailable_error())?;
        let mut next = guard.clone();
        let index = next
            .jobs
            .iter()
            .position(|job| job.id == id)
            .ok_or_else(|| AppError::new("MEDIA_JOB_NOT_FOUND", "媒体任务不存在"))?;
        if next.jobs[index].status != MediaJobStatus::Running {
            return Err(AppError::new(
                "MEDIA_JOB_INVALID_TRANSITION",
                "媒体任务当前状态不允许该操作",
            ));
        }
        next.jobs[index].status = MediaJobStatus::Queued;
        next.jobs[index].stage = "queued".into();
        next.jobs[index].percent = 0.0;
        let restored = next.jobs[index].clone();
        persist_jobs(&self.store_dir, &next.jobs)?;
        *guard = next;
        Ok(restored)
    }

    pub fn update_progress(
        &self,
        id: &str,
        stage: String,
        percent: f64,
    ) -> Result<MediaJob, AppError> {
        let mut guard = self.state.lock().map_err(|_| manager_unavailable_error())?;
        let mut next = guard.clone();
        let job = next
            .jobs
            .iter_mut()
            .find(|job| job.id == id)
            .ok_or_else(|| AppError::new("MEDIA_JOB_NOT_FOUND", "媒体任务不存在"))?;
        if job.status != MediaJobStatus::Running
            && !(job.status == MediaJobStatus::Paused
                && job.pause_origin == Some(MediaJobPauseOrigin::Running))
        {
            return Err(AppError::new(
                "MEDIA_JOB_INVALID_TRANSITION",
                "媒体任务当前状态不允许更新进度",
            ));
        }
        job.stage = stage;
        job.percent = percent.clamp(0.0, 99.9);
        let updated = job.clone();
        persist_jobs(&self.store_dir, &next.jobs)?;
        *guard = next;
        Ok(updated)
    }

    pub fn retry_merge(
        &self,
        id: &str,
        revalidated: ValidatedMergeRequest,
    ) -> Result<MediaJob, AppError> {
        let mut guard = self.state.lock().map_err(|_| manager_unavailable_error())?;
        let mut next = guard.clone();
        let index = next
            .jobs
            .iter()
            .position(|job| job.id == id)
            .ok_or_else(|| AppError::new("MEDIA_JOB_NOT_FOUND", "媒体任务不存在"))?;
        let original = next.jobs[index].merge_request.as_ref().ok_or_else(|| {
            AppError::new(
                "MEDIA_JOB_RETRY_UNAVAILABLE",
                "媒体任务缺少可重试的原始请求",
            )
        })?;
        if original != &revalidated {
            return Err(AppError::new(
                "MEDIA_INPUT_CHANGED",
                "合并输入已移动或发生变化",
            ));
        }
        let dedupe_key = next.jobs[index].dedupe_key.clone();
        if next.jobs.iter().enumerate().any(|(other_index, job)| {
            other_index != index
                && job.dedupe_key == dedupe_key
                && matches!(
                    job.status,
                    MediaJobStatus::Queued | MediaJobStatus::Running | MediaJobStatus::Paused
                )
        }) {
            return Err(AppError::new(
                "MEDIA_JOB_ALREADY_ACTIVE",
                "同一媒体任务已在队列中或正在运行",
            ));
        }
        apply_transition(&mut next.jobs[index], MediaJobTransition::Retry)?;
        let updated = next.jobs.remove(index);
        next.jobs.push(updated.clone());
        persist_jobs(&self.store_dir, &next.jobs)?;
        *guard = next;
        Ok(updated)
    }

    pub fn retry_ai(
        &self,
        id: &str,
        revalidated: ValidatedAIJobRequest,
    ) -> Result<MediaJob, AppError> {
        let mut guard = self.state.lock().map_err(|_| manager_unavailable_error())?;
        let mut next = guard.clone();
        let index = next
            .jobs
            .iter()
            .position(|job| job.id == id)
            .ok_or_else(|| AppError::new("MEDIA_JOB_NOT_FOUND", "媒体任务不存在"))?;
        let original = next.jobs[index].ai_request.as_ref().ok_or_else(|| {
            AppError::new(
                "MEDIA_JOB_RETRY_UNAVAILABLE",
                "媒体任务缺少可重试的 AI 请求",
            )
        })?;
        if original != &revalidated {
            return Err(AppError::new(
                "MEDIA_INPUT_CHANGED",
                "AI 处理输入已发生变化",
            ));
        }
        let dedupe_key = next.jobs[index].dedupe_key.clone();
        if next.jobs.iter().enumerate().any(|(other_index, job)| {
            other_index != index
                && job.dedupe_key == dedupe_key
                && matches!(
                    job.status,
                    MediaJobStatus::Queued | MediaJobStatus::Running | MediaJobStatus::Paused
                )
        }) {
            return Err(AppError::new(
                "MEDIA_JOB_ALREADY_ACTIVE",
                "同一媒体任务已在队列中或正在运行",
            ));
        }
        apply_transition(&mut next.jobs[index], MediaJobTransition::Retry)?;
        let updated = next.jobs.remove(index);
        next.jobs.push(updated.clone());
        persist_jobs(&self.store_dir, &next.jobs)?;
        *guard = next;
        Ok(updated)
    }

    pub fn cancel(&self, id: &str) -> Result<MediaJob, AppError> {
        self.update(id, MediaJobTransition::Cancel)
    }

    pub fn pause_queued(&self, id: &str) -> Result<MediaJob, AppError> {
        self.update(id, MediaJobTransition::PauseQueued)
    }

    pub fn pause_running(&self, id: &str) -> Result<MediaJob, AppError> {
        self.update(id, MediaJobTransition::PauseRunning)
    }

    pub fn resume_queued(&self, id: &str) -> Result<MediaJob, AppError> {
        self.update(id, MediaJobTransition::ResumeQueued)
    }

    pub fn resume_running(&self, id: &str) -> Result<MediaJob, AppError> {
        self.update(id, MediaJobTransition::ResumeRunning)
    }

    pub fn remove(&self, id: &str) -> Result<MediaJob, AppError> {
        let mut guard = self.state.lock().map_err(|_| manager_unavailable_error())?;
        let mut next = guard.clone();
        let index = next
            .jobs
            .iter()
            .position(|job| job.id == id)
            .ok_or_else(|| AppError::new("MEDIA_JOB_NOT_FOUND", "媒体任务不存在"))?;
        let removed = next.jobs.remove(index);
        persist_jobs(&self.store_dir, &next.jobs)?;
        *guard = next;
        Ok(removed)
    }

    pub fn retry(&self, id: &str) -> Result<MediaJob, AppError> {
        self.update(id, MediaJobTransition::Retry)
    }

    pub fn update(&self, id: &str, transition: MediaJobTransition) -> Result<MediaJob, AppError> {
        let mut guard = self.state.lock().map_err(|_| manager_unavailable_error())?;
        let mut next = guard.clone();
        let job_index = next
            .jobs
            .iter()
            .position(|job| job.id == id)
            .ok_or_else(|| AppError::new("MEDIA_JOB_NOT_FOUND", "媒体任务不存在"))?;
        let is_retry = matches!(&transition, MediaJobTransition::Retry);

        apply_transition(&mut next.jobs[job_index], transition)?;
        if is_retry {
            let dedupe_key = &next.jobs[job_index].dedupe_key;
            let duplicate_active = next.jobs.iter().enumerate().any(|(index, job)| {
                index != job_index
                    && job.dedupe_key == *dedupe_key
                    && matches!(
                        job.status,
                        MediaJobStatus::Queued | MediaJobStatus::Running | MediaJobStatus::Paused
                    )
            });
            if duplicate_active {
                return Err(AppError::new(
                    "MEDIA_JOB_ALREADY_ACTIVE",
                    "同一媒体任务已在队列中或正在运行",
                ));
            }
        }
        let mut updated = next.jobs[job_index].clone();
        if is_retry {
            updated = next.jobs.remove(job_index);
            next.jobs.push(updated.clone());
        }
        persist_jobs(&self.store_dir, &next.jobs)?;
        *guard = next;
        Ok(updated)
    }
}

fn apply_transition(job: &mut MediaJob, transition: MediaJobTransition) -> Result<(), AppError> {
    match (job.status, transition) {
        (MediaJobStatus::Queued, MediaJobTransition::Start) => {
            job.status = MediaJobStatus::Running;
            job.stage = "running".into();
            job.pause_origin = None;
        }
        (MediaJobStatus::Queued, MediaJobTransition::PauseQueued) => {
            job.status = MediaJobStatus::Paused;
            job.stage = "paused".into();
            job.pause_origin = Some(MediaJobPauseOrigin::Queued);
        }
        (MediaJobStatus::Running, MediaJobTransition::PauseRunning) => {
            job.status = MediaJobStatus::Paused;
            job.stage = "paused".into();
            job.pause_origin = Some(MediaJobPauseOrigin::Running);
        }
        (MediaJobStatus::Paused, MediaJobTransition::ResumeQueued)
            if job.pause_origin == Some(MediaJobPauseOrigin::Queued) =>
        {
            job.status = MediaJobStatus::Queued;
            job.stage = "queued".into();
            job.pause_origin = None;
        }
        (MediaJobStatus::Paused, MediaJobTransition::ResumeRunning)
            if job.pause_origin == Some(MediaJobPauseOrigin::Running) =>
        {
            job.status = MediaJobStatus::Running;
            job.stage = "running".into();
            job.pause_origin = None;
        }
        (
            status,
            MediaJobTransition::Complete {
                output_path,
                outputs,
            },
        ) if status == MediaJobStatus::Running
            || (status == MediaJobStatus::Paused
                && job.pause_origin == Some(MediaJobPauseOrigin::Running)) =>
        {
            job.status = MediaJobStatus::Completed;
            job.stage = "completed".into();
            job.percent = 100.0;
            job.output_path = Some(output_path);
            job.outputs = outputs;
            job.error_code = None;
            job.error_message = None;
            job.pause_origin = None;
        }
        (status, MediaJobTransition::Fail { code, message })
            if status == MediaJobStatus::Running
                || (status == MediaJobStatus::Paused
                    && job.pause_origin == Some(MediaJobPauseOrigin::Running)) =>
        {
            job.status = MediaJobStatus::Failed;
            job.stage = "failed".into();
            job.error_code = Some(code);
            job.error_message = Some(message);
            job.pause_origin = None;
        }
        (status, MediaJobTransition::Cancel)
            if status == MediaJobStatus::Running
                || (status == MediaJobStatus::Paused
                    && job.pause_origin == Some(MediaJobPauseOrigin::Running)) =>
        {
            job.status = MediaJobStatus::Cancelled;
            job.stage = "cancelled".into();
            job.pause_origin = None;
        }
        (
            MediaJobStatus::Failed | MediaJobStatus::Cancelled | MediaJobStatus::Interrupted,
            MediaJobTransition::Retry,
        ) => {
            job.status = MediaJobStatus::Queued;
            job.stage = "queued".into();
            job.percent = 0.0;
            job.output_path = None;
            job.error_code = None;
            job.error_message = None;
            job.outputs.clear();
            job.completion_notified_at = None;
            job.failure_notified_at = None;
            job.pause_origin = None;
        }
        _ => {
            return Err(AppError::new(
                "MEDIA_JOB_INVALID_TRANSITION",
                "媒体任务当前状态不允许该操作",
            ));
        }
    }
    Ok(())
}

fn persist_jobs(store_dir: &Path, jobs: &[MediaJob]) -> Result<(), AppError> {
    let persisted = PersistedMediaJobs {
        version: MEDIA_JOBS_VERSION,
        jobs: jobs.to_vec(),
    };
    let temporary_path = store_dir.join(TEMP_STORE_NAME);
    let canonical_path = store_dir.join(STORE_NAME);
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary_path)
        .map_err(storage_error)?;
    let mut writer = BufWriter::new(file);
    if let Err(error) = serde_json::to_writer_pretty(&mut writer, &persisted) {
        let _ = fs::remove_file(&temporary_path);
        return Err(storage_error(error));
    }
    if let Err(error) = writer.flush() {
        let _ = fs::remove_file(&temporary_path);
        return Err(storage_error(error));
    }
    if let Err(error) = writer.get_ref().sync_all() {
        let _ = fs::remove_file(&temporary_path);
        return Err(storage_error(error));
    }
    drop(writer);
    replace_file(&temporary_path, &canonical_path).map_err(storage_error)?;
    sync_directory(store_dir).map_err(storage_error)
}

fn quarantine_corrupt_store(store_dir: &Path, store_path: &Path) -> Result<(), AppError> {
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut suffix = 0u64;
    let quarantine_path = loop {
        let timestamp = unix.saturating_add(suffix);
        let candidate = store_dir.join(format!("media-jobs.corrupt.{timestamp}.json"));
        if !candidate.exists() {
            break candidate;
        }
        suffix = suffix.saturating_add(1);
    };
    fs::rename(store_path, quarantine_path).map_err(storage_error)?;
    sync_directory(store_dir).map_err(storage_error)
}

fn next_job_id() -> String {
    let unix_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = JOB_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("media-{unix_nanos}-{sequence}")
}

fn storage_error(cause: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "MEDIA_JOB_STORAGE_IO",
        "媒体任务存储失败",
        cause.to_string(),
    )
}

fn manager_unavailable_error() -> AppError {
    AppError::new("MEDIA_JOB_MANAGER_UNAVAILABLE", "媒体任务管理器暂不可用")
}

#[cfg(test)]
mod notification_tests {
    use super::*;
    use crate::media::{InputSnapshot, MediaJobKind};

    #[test]
    fn terminal_notification_claim_is_persisted_once_and_retry_clears_it() {
        let root = std::env::temp_dir().join(format!(
            "hongguo-media-notification-{}-{}",
            std::process::id(),
            JOB_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let manager = MediaJobManager::load(&root).unwrap();
        let job = manager
            .enqueue(MediaJobRequest {
                dedupe_key: "notification-test".into(),
                kind: MediaJobKind::Merge,
                inputs: vec![InputSnapshot {
                    path: root.join("episode.mp4"),
                    size_bytes: 1,
                }],
            })
            .unwrap();
        manager.claim_oldest_queued().unwrap();
        manager
            .update(
                &job.id,
                MediaJobTransition::Fail {
                    code: "TEST_FAILURE".into(),
                    message: "测试失败".into(),
                },
            )
            .unwrap();
        let first = manager.mark_notified(&job.id, false).unwrap();
        let second = manager.mark_notified(&job.id, false).unwrap();
        assert_eq!(first.failure_notified_at, second.failure_notified_at);
        drop(manager);
        let reloaded = MediaJobManager::load(&root).unwrap();
        assert_eq!(
            reloaded.job(&job.id).unwrap().failure_notified_at,
            first.failure_notified_at
        );
        let retried = reloaded.retry(&job.id).unwrap();
        assert!(retried.failure_notified_at.is_none());
        let _ = fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::media::{InputSnapshot, MediaJobKind, MediaJobPauseOrigin};

    fn test_manager(label: &str) -> (PathBuf, MediaJobManager) {
        let root = std::env::temp_dir().join(format!(
            "hongguo-media-lifecycle-{label}-{}-{}",
            std::process::id(),
            JOB_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let manager = MediaJobManager::load(&root).unwrap();
        (root, manager)
    }

    fn enqueue(manager: &MediaJobManager, key: &str) -> MediaJob {
        manager
            .enqueue(MediaJobRequest {
                dedupe_key: key.into(),
                kind: MediaJobKind::Merge,
                inputs: vec![InputSnapshot {
                    path: PathBuf::from("episode.mp4"),
                    size_bytes: 1,
                }],
            })
            .unwrap()
    }

    #[test]
    fn queued_pause_survives_reload_and_resume_returns_to_queue() {
        let (root, manager) = test_manager("queued-pause");
        let job = enqueue(&manager, "queued-pause");
        let paused = manager.pause_queued(&job.id).unwrap();
        assert_eq!(paused.status, MediaJobStatus::Paused);
        assert_eq!(paused.pause_origin, Some(MediaJobPauseOrigin::Queued));
        drop(manager);

        let reloaded = MediaJobManager::load(&root).unwrap();
        let persisted = reloaded.job(&job.id).unwrap();
        assert_eq!(persisted.status, MediaJobStatus::Paused);
        assert_eq!(persisted.pause_origin, Some(MediaJobPauseOrigin::Queued));
        let resumed = reloaded.resume_queued(&job.id).unwrap();
        assert_eq!(resumed.status, MediaJobStatus::Queued);
        assert_eq!(resumed.pause_origin, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn running_pause_accepts_last_progress_and_recovers_as_interrupted() {
        let (root, manager) = test_manager("running-pause");
        let job = enqueue(&manager, "running-pause");
        manager.claim_oldest_queued().unwrap();
        let paused = manager.pause_running(&job.id).unwrap();
        assert_eq!(paused.pause_origin, Some(MediaJobPauseOrigin::Running));
        let progressed = manager
            .update_progress(&job.id, "merging".into(), 48.0)
            .unwrap();
        assert_eq!(progressed.percent, 48.0);
        drop(manager);

        let reloaded = MediaJobManager::load(&root).unwrap();
        let recovered = reloaded.job(&job.id).unwrap();
        assert_eq!(recovered.status, MediaJobStatus::Interrupted);
        assert_eq!(recovered.pause_origin, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remove_deletes_record_from_persisted_snapshot() {
        let (root, manager) = test_manager("remove");
        let job = enqueue(&manager, "remove");
        let removed = manager.remove(&job.id).unwrap();
        assert_eq!(removed.id, job.id);
        assert_eq!(
            manager.job(&job.id).unwrap_err().code,
            "MEDIA_JOB_NOT_FOUND"
        );
        drop(manager);
        let reloaded = MediaJobManager::load(&root).unwrap();
        assert!(reloaded.snapshot().jobs.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn paused_job_blocks_duplicate_enqueue() {
        let (root, manager) = test_manager("dedupe");
        let job = enqueue(&manager, "same-key");
        manager.pause_queued(&job.id).unwrap();
        let error = manager
            .enqueue(MediaJobRequest {
                dedupe_key: "same-key".into(),
                kind: MediaJobKind::Merge,
                inputs: Vec::new(),
            })
            .unwrap_err();
        assert_eq!(error.code, "MEDIA_JOB_ALREADY_ACTIVE");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn running_origin_pause_can_finish_at_the_process_exit_boundary() {
        let (root, manager) = test_manager("paused-terminal");
        let job = enqueue(&manager, "paused-terminal");
        manager.claim_oldest_queued().unwrap();
        manager.pause_running(&job.id).unwrap();

        let completed = manager
            .update(
                &job.id,
                MediaJobTransition::Complete {
                    output_path: root.join("合并视频/output.mp4"),
                    outputs: Vec::new(),
                },
            )
            .unwrap();
        assert_eq!(completed.status, MediaJobStatus::Completed);
        assert_eq!(completed.pause_origin, None);
        let _ = fs::remove_dir_all(root);
    }
}
