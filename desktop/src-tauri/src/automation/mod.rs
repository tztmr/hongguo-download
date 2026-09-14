mod credentials;
mod discovery;
mod metadata;
pub mod model;
mod pipeline;
mod runner;
pub mod source;
mod storage;
#[cfg(test)]
mod tests;

use crate::{AppError, AppState};
use model::*;
use serde_json::Value;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{AppHandle, Manager, State};

pub struct Service {
    path: PathBuf,
    state: Mutex<Snapshot>,
    busy: Mutex<HashSet<String>>,
    scanning: AtomicBool,
}
impl Service {
    pub fn load(path: PathBuf) -> Result<Arc<Self>, AppError> {
        let mut snapshot = storage::load(&path)?;
        if let Some(types) = snapshot
            .config
            .as_mut()
            .and_then(|c| c["types"].as_array_mut())
        {
            if types.iter().any(|v| v.as_str() == Some("真人剧")) {
                types.retain(|v| v.as_str() != Some("真人剧"));
                snapshot.cursor.clear();
                snapshot.cursor_type = 0;
                if types.is_empty() {
                    if snapshot.mode == Mode::Running {
                        snapshot.mode = Mode::Paused;
                    }
                    snapshot.warning = "自动追剧已取消真人剧，请选择漫剧或 AI剧后保存设置".into();
                }
            }
        }
        if let Some(config) = snapshot.config.as_ref() {
            if !flag(config, "resume") && snapshot.mode == Mode::Running {
                snapshot.mode = Mode::Paused;
            }
        }
        for job in &mut snapshot.jobs {
            if job.manual_skip {
                job.skip_manually();
                continue;
            }
            if job.status == Status::Completed
                && job.cleanup_version == 0
                && flag(&job.config, "deleteEpisodes")
                && flag(&job.config, "deleteFinal")
                && job.main_done
                && (!job.shorts_required() || job.short_done)
            {
                job.stage = "cleanup".into();
                job.status = Status::Pending;
                job.retry_at = 0;
                job.message = "升级后补做目录清理，保留上传记录，不重复上传".into();
            }
            if job.status == Status::Failed {
                job.status = Status::Pending;
                job.attempts = 0;
                job.retry_at = 0;
                job.retry_ready = true;
                job.message = format!(
                    "已加入自动恢复队列，复用已完成文件；上次错误：{}",
                    job.message
                );
            }
            if job.status == Status::Working {
                job.status = Status::Pending;
                job.message = "应用重启，核对上次阶段结果后继续".into();
            }
        }
        pipeline::normalize_group(&mut snapshot);
        snapshot.next_scan = 0;
        storage::save(&path, &snapshot)?;
        Ok(Arc::new(Self {
            path,
            state: Mutex::new(snapshot),
            busy: Mutex::new(HashSet::new()),
            scanning: AtomicBool::new(false),
        }))
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn running(&self) -> bool {
        self.snapshot().mode == Mode::Running
    }
    pub(crate) fn is_skipped(&self, id: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .jobs
            .iter()
            .any(|j| j.id == id && j.manual_skip)
    }
    fn transaction<T>(
        &self,
        f: impl FnOnce(&mut Snapshot) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let mut guard = self.state.lock().map_err(|_| unavailable())?;
        let mut next = guard.clone();
        let free_before = pipeline::free_slots(&next);
        let result = f(&mut next)?;
        if next.mode == Mode::Running && pipeline::free_slots(&next) > free_before {
            next.next_scan = 0;
            next.cursor.clear();
        }
        pipeline::normalize_group(&mut next);
        if let Err(error) = storage::save(&self.path, &next) {
            guard.mode = Mode::Paused;
            guard.warning = error.message.clone();
            return Err(error);
        }
        *guard = next;
        Ok(result)
    }
    pub(crate) fn checkpoint(&self, job: &Task) -> Result<(), AppError> {
        self.transaction(|s| {
            let target = s
                .jobs
                .iter_mut()
                .find(|j| j.id == job.id)
                .ok_or_else(unavailable)?;
            target.accept_progress(job);
            Ok(())
        })
    }
    fn save_config(
        &self,
        config: Value,
        secrets: credentials::Updates,
    ) -> Result<Snapshot, AppError> {
        let config = validate_config(config)?;
        if self.snapshot().mode == Mode::Running
            || !self.busy.lock().map_err(|_| unavailable())?.is_empty()
        {
            return Err(AppError::new(
                "AUTOMATION_RUNNING",
                "请先暂停，待当前阶段收尾后再保存设置",
            ));
        }
        let status = credentials::update(&config, secrets)?;
        self.transaction(|s| {
            s.config = Some(config);
            s.discovery = discovery::Discovery::default();
            s.discovery_pending.clear();
            s.key_status = status;
            s.log(None, "设置已保存；点击启动才开始自动处理");
            Ok(())
        })?;
        Ok(self.snapshot())
    }
    fn activate(&self, app: &AppHandle) -> Result<Snapshot, AppError> {
        let state = app.state::<AppState>();
        let saved = self.snapshot();
        let c = saved
            .config
            .as_ref()
            .ok_or_else(|| AppError::new("AUTOMATION_NOT_CONFIGURED", "请先保存设置"))?;
        validate_config(c.clone())?;
        let yt = state.youtube.snapshot();
        if !yt
            .channels
            .iter()
            .any(|a| a.channel_id == text(c, "channel"))
        {
            return Err(AppError::new(
                "AUTH_REQUIRED",
                "目标频道尚未授权，请先在设置中连接 YouTube",
            ));
        }
        if yt.active_channel_id.as_deref() != Some(text(c, "channel")) {
            return Err(AppError::new(
                "AUTOMATION_CHANNEL_MISMATCH",
                "请在设置中切换到自动追剧所选频道，再启动",
            ));
        }
        let root = state
            .settings
            .lock()
            .map_err(|_| unavailable())?
            .save_dir
            .clone();
        std::fs::create_dir_all(&root)
            .map_err(|_| AppError::new("AUTOMATION_DIRECTORY_INVALID", "下载目录无法创建"))?;
        runner::check_disk(&root, number(c, "minDisk", 20))?;
        self.transaction(|s| {
            s.mode = Mode::Running;
            s.next_scan = 0;
            s.cursor.clear();
            s.cursor_type = 0;
            s.warning.clear();
            for job in &mut s.jobs {
                if job.stage == "inspect"
                    && job.files.is_empty()
                    && !matches!(job.status, Status::Skipped | Status::Completed)
                    && text(&job.config, "channel") == text(c, "channel")
                {
                    job.config = c.clone();
                    if job.status == Status::Observing {
                        job.status = Status::Pending;
                        job.retry_at = 0;
                    }
                }
            }
            s.log(
                None,
                "自动追剧已启动；按已保存频道和可见性执行，应用需保持运行",
            );
            Ok(())
        })?;
        Ok(self.snapshot())
    }
    fn control(&self, action: &str) -> Result<Snapshot, AppError> {
        self.transaction(|s| {
            match action {
                "pause" => {
                    s.mode = Mode::Paused;
                    s.log(None, "已暂停扫描和后续阶段；当前网络请求收尾后暂停");
                }
                "stop" => {
                    s.mode = Mode::Stopped;
                    s.log(None, "已停止自动追剧；保留进度和文件，启动后可续接");
                }
                "scan" => {
                    if s.mode != Mode::Running {
                        return Err(AppError::new("AUTOMATION_NOT_RUNNING", "请先启动自动追剧"));
                    }
                    if !pipeline::can_scan(s) {
                        return Err(AppError::new(
                            "AUTOMATION_GROUP_ACTIVE",
                            "当前 10 个名额已满，完成或跳过一部后自动监听补位",
                        ));
                    }
                    s.next_scan = 0;
                }
                _ => {
                    return Err(AppError::new(
                        "AUTOMATION_INVALID_ACTION",
                        "不支持的运行操作",
                    ));
                }
            }
            Ok(())
        })?;
        Ok(self.snapshot())
    }
    fn review(&self, id: &str, action: &str) -> Result<Snapshot, AppError> {
        if action != "skip" && self.busy.lock().map_err(|_| unavailable())?.contains(id) {
            return Err(AppError::new(
                "AUTOMATION_BUSY",
                "当前阶段仍在收尾，请稍后重试",
            ));
        }
        self.transaction(|s| {
            let j = s
                .jobs
                .iter_mut()
                .find(|j| j.id == id)
                .ok_or_else(|| AppError::new("AUTOMATION_JOB_NOT_FOUND", "任务不存在"))?;
            match action {
                "continue" if j.status == Status::Review => {
                    j.allow_duplicate = true;
                    j.status = Status::Pending;
                    j.message = "已确认继续；仍检查缺集和真实视频格式".into();
                }
                "skip" if !matches!(j.status, Status::Completed | Status::Skipped) => {
                    j.skip_manually();
                }
                "retry" if matches!(j.status, Status::Failed | Status::Observing) => {
                    j.status = Status::Pending;
                    j.attempts = 0;
                    j.retry_at = 0;
                    j.retry_ready = true;
                    j.message = "等待重试，复用已完成阶段".into();
                }
                _ => {
                    return Err(AppError::new(
                        "AUTOMATION_INVALID_ACTION",
                        "此任务当前不能执行该操作",
                    ));
                }
            }
            s.log(Some(id.into()), format!("任务操作：{action}"));
            Ok(())
        })?;
        Ok(self.snapshot())
    }
    pub fn spawn(self: &Arc<Self>, app: AppHandle) {
        // A macOS Keychain prompt may wait for the user indefinitely. Never open
        // the vault on the UI setup thread or while holding the state mutex.
        let owner = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if let Some(config) = owner.snapshot().config {
                let status = credentials::status(&config);
                let _ = owner.transaction(|snapshot| {
                    if snapshot.config.as_ref() == Some(&config) {
                        snapshot.key_status = status;
                    }
                    Ok(())
                });
            }
        });
        let service = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                if service.running() {
                    let s = service.snapshot();
                    if s.config.is_some() {
                        if pipeline::can_scan(&s)
                            && now() >= s.next_scan
                            && service
                                .scanning
                                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                                .is_ok()
                        {
                            let a = app.clone();
                            let owner = service.clone();
                            tauri::async_runtime::spawn(async move {
                                let outcome = runner::scan(&a, &owner).await;
                                if let Err(e) = outcome {
                                    let _ = owner.transaction(|s| {
                                        s.warning = format!("扫描失败：{}", e.message);
                                        s.next_scan = now() + 60;
                                        s.log(None, s.warning.clone());
                                        Ok(())
                                    });
                                }
                                owner.scanning.store(false, Ordering::SeqCst);
                            });
                        }
                        let ready = pipeline::select(
                            &s,
                            &service.busy.lock().unwrap_or_else(|e| e.into_inner()),
                            now(),
                        );
                        for id in ready {
                            let mut busy = service.busy.lock().unwrap_or_else(|e| e.into_inner());
                            if !busy.insert(id.clone()) {
                                continue;
                            }
                            drop(busy);
                            let Some(mut task) =
                                service.snapshot().jobs.into_iter().find(|j| j.id == id)
                            else {
                                service.busy.lock().unwrap().remove(&id);
                                continue;
                            };
                            if !service.running() || task.terminal() || task.retry_at > now() {
                                service.busy.lock().unwrap().remove(&id);
                                continue;
                            }
                            // Reserve this drama's group slot durably before any
                            // download side effect. First-episode retries and
                            // restarts must not admit a fresh batch each time.
                            if task.stage == "download" {
                                task.download_admitted = true;
                            }
                            task.status = Status::Working;
                            if service.checkpoint(&task).is_err() {
                                service.busy.lock().unwrap().remove(&task.id);
                                break;
                            }
                            let a = app.clone();
                            let owner = service.clone();
                            tauri::async_runtime::spawn(async move {
                                let before = task.message.clone();
                                let outcome = runner::advance(&a, &owner, &mut task).await;
                                if let Err(e) = outcome {
                                    task.attempts = task.attempts.saturating_add(1);
                                    task.message = e.message.clone();
                                    if matches!(
                                        e.code.as_str(),
                                        "YOUTUBE_DUPLICATE_FOUND"
                                            | "UPLOAD_DUPLICATE_REVIEW_REQUIRED"
                                            | "AUTOMATION_LOCAL_REVIEW"
                                    ) {
                                        task.status = Status::Review;
                                    } else if e.code == "AUTOMATION_DISK_LOW" {
                                        task.status = Status::Observing;
                                        task.retry_at = now() + 60;
                                        task.attempts = 0;
                                    } else if e.code == "AUTOMATION_PAUSED" {
                                        task.status = Status::Pending;
                                    } else {
                                        task.defer_retry();
                                    }
                                }
                                task.updated_at = now();
                                if task.status == Status::Working {
                                    task.status = Status::Pending;
                                }
                                let _ = owner.transaction(|s| {
                                    if let Some(j) = s.jobs.iter_mut().find(|j| j.id == task.id) {
                                        j.accept_progress(&task);
                                        task = j.clone();
                                    }
                                    if task.message != before {
                                        s.log(Some(task.id.clone()), task.message.clone());
                                    }
                                    Ok(())
                                });
                                if matches!(task.status, Status::Completed | Status::Failed)
                                    && flag(&task.config, "notify")
                                {
                                    use tauri_plugin_notification::NotificationExt;
                                    let _ = a
                                        .notification()
                                        .builder()
                                        .title("自动追剧")
                                        .body(format!("{}：{}", task.title, task.message))
                                        .show();
                                }
                                if !owner.running() {
                                    runner::pause_owned(&a, &task);
                                }
                                if task.manual_skip {
                                    runner::cancel_owned(&a, &task);
                                }
                                owner
                                    .busy
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .remove(&task.id);
                            });
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });
    }
}
fn unavailable() -> AppError {
    AppError::new("AUTOMATION_UNAVAILABLE", "自动追剧状态暂不可用")
}

#[tauri::command]
pub(crate) fn get_automation_snapshot(state: State<AppState>) -> Snapshot {
    state.automation.snapshot()
}
#[tauri::command]
pub(crate) async fn save_automation_settings(
    state: State<'_, AppState>,
    config: Value,
    secrets: Option<credentials::Updates>,
) -> Result<Snapshot, AppError> {
    let service = state.automation.clone();
    crate::run_blocking(move || service.save_config(config, secrets.unwrap_or_default())).await
}
#[tauri::command]
pub(crate) fn start_automation(
    app: AppHandle,
    state: State<AppState>,
) -> Result<Snapshot, AppError> {
    state.automation.activate(&app)
}
#[tauri::command]
pub(crate) fn control_automation(
    app: AppHandle,
    state: State<AppState>,
    action: String,
) -> Result<Snapshot, AppError> {
    if action == "resume" {
        return state.automation.activate(&app);
    }
    let result = state.automation.control(&action)?;
    if action == "pause" || action == "stop" {
        for job in &result.jobs {
            runner::pause_owned(&app, job);
        }
    }
    Ok(result)
}
#[tauri::command]
pub(crate) fn review_automation_job(
    app: AppHandle,
    state: State<AppState>,
    job_id: String,
    action: String,
) -> Result<Snapshot, AppError> {
    let result = state.automation.review(&job_id, &action)?;
    if action == "skip" {
        if let Some(job) = result.jobs.iter().find(|j| j.id == job_id) {
            runner::cancel_owned(&app, job);
        }
    }
    Ok(result)
}
