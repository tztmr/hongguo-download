use super::{model::*, source, Service};
use crate::{
    media::model::{MediaJobOutputKind, MediaJobScope, MediaJobStatus, MergeMode, StartMergeInput},
    media::{MediaJob, MediaJobKind, MediaTools, StartAIJobRequest, StartMergeRequest},
    youtube::{
        duplicates::{DuplicateQuery, MatchConfidence, UploadIdentity},
        format::UploadFormat,
        models::{PrivacyStatus, UploadIntent, YouTubeJobStatus},
        subtitles::SubtitleRequest,
    },
    AppError, AppState,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
use tauri::{AppHandle, Manager};

async fn api(app: &AppHandle, path: String) -> Result<Value, AppError> {
    crate::api_get(app.state::<AppState>(), path).await
}
pub fn check_disk(root: &Path, min_gb: u64) -> Result<(), AppError> {
    let bytes = fs2::available_space(root)
        .map_err(|_| AppError::new("AUTOMATION_DISK_UNKNOWN", "无法读取下载磁盘剩余空间"))?;
    if bytes < min_gb.saturating_mul(1024 * 1024 * 1024) {
        return Err(AppError::new(
            "AUTOMATION_DISK_LOW",
            format!("磁盘剩余空间低于 {min_gb} GB，保留进度等待释放空间"),
        ));
    }
    Ok(())
}
fn ensure_running(service: &Service) -> Result<(), AppError> {
    if service.running() {
        Ok(())
    } else {
        Err(AppError::new(
            "AUTOMATION_PAUSED",
            "运行已暂停，当前阶段结果已保留",
        ))
    }
}
fn ensure_task_running(service: &Service, id: &str) -> Result<(), AppError> {
    ensure_running(service)?;
    if service.is_skipped(id) {
        return Err(AppError::new(
            "AUTOMATION_SKIPPED",
            "任务已手动跳过，停止后续处理",
        ));
    }
    Ok(())
}
fn tools() -> Result<MediaTools, AppError> {
    let exe = std::env::current_exe().map_err(|_| invalid_file())?;
    MediaTools::from_resource_root(exe.parent().ok_or_else(invalid_file)?)
}
fn invalid_file() -> AppError {
    AppError::new(
        "AUTOMATION_FILE_INVALID",
        "自动任务文件缺失或发生变化，保留现场等待检查",
    )
}
fn safe_root(root: &Path) -> Result<(), AppError> {
    for p in root.ancestors() {
        if p.is_symlink() {
            return Err(invalid_file());
        }
        #[cfg(windows)]
        if let Ok(metadata) = fs::symlink_metadata(p) {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(invalid_file());
            }
        }
    }
    Ok(())
}
fn own(task: &mut Task, path: &Path) -> Result<(), AppError> {
    safe_root(path)?;
    let path = fs::canonicalize(path).map_err(|_| invalid_file())?;
    let root = fs::canonicalize(&task.root).map_err(|_| invalid_file())?;
    if !path.starts_with(root) {
        return Err(invalid_file());
    }
    let file = identity(&path)?;
    task.owned.retain(|p| p.path != path);
    task.owned.push(file);
    Ok(())
}
fn identity(path: &Path) -> Result<OwnedFile, AppError> {
    let mut f = fs::File::open(path).map_err(|_| invalid_file())?;
    let size = f.metadata().map_err(|_| invalid_file())?.len();
    let mut digest = Sha256::new();
    let mut buf = [0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|_| invalid_file())?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
    }
    Ok(OwnedFile {
        path: path.into(),
        size,
        hash: format!("{:x}", digest.finalize()),
    })
}

fn admit_group(
    snapshot: &mut Snapshot,
    candidates: Vec<source::Candidate>,
    config: &Value,
    save_dir: &Path,
) -> usize {
    if !super::pipeline::can_scan(snapshot) {
        return 0;
    }
    let capacity = super::pipeline::free_slots(snapshot);
    let mut count = 0;
    for candidate in candidates {
        if count == capacity {
            break;
        }
        let task = Task::new(candidate, config.clone(), save_dir.to_path_buf());
        if snapshot.jobs.iter().any(|j| j.id == task.id) {
            continue;
        }
        snapshot.jobs.push(task);
        count += 1;
    }
    count
}

fn buffer_candidates(
    snapshot: &mut Snapshot,
    candidates: Vec<source::Candidate>,
    config: &Value,
    root: &Path,
) -> usize {
    let count = admit_group(snapshot, candidates.clone(), config, root);
    let mut seen = std::collections::HashSet::new();
    snapshot.discovery_pending = candidates
        .into_iter()
        .filter(|c| {
            let id = Task::new(c.clone(), config.clone(), root.to_path_buf()).id;
            !snapshot.jobs.iter().any(|j| j.id == id) && seen.insert(id)
        })
        .take(100)
        .collect();
    snapshot.scan_summary.buffered = snapshot.discovery_pending.len();
    count
}

pub async fn scan(app: &AppHandle, service: &Arc<Service>) -> Result<(), AppError> {
    let snapshot = service.snapshot();
    if !super::pipeline::can_scan(&snapshot) {
        return Ok(());
    }
    let config = snapshot
        .config
        .clone()
        .ok_or_else(|| AppError::new("AUTOMATION_NOT_CONFIGURED", "请先保存设置"))?;
    let save_dir = app
        .state::<AppState>()
        .settings
        .lock()
        .map_err(|_| invalid_file())?
        .save_dir
        .clone();
    fs::create_dir_all(&save_dir).map_err(|_| invalid_file())?;
    let save_dir = fs::canonicalize(save_dir).map_err(|_| invalid_file())?;
    if !snapshot.discovery_pending.is_empty() {
        return service.transaction(|s| {
            if !super::pipeline::can_scan(s) || s.config.as_ref() != Some(&config) {
                return Ok(());
            }
            let pending = std::mem::take(&mut s.discovery_pending);
            let count = buffer_candidates(s, pending, &config, &save_dir);
            s.scan_summary.added += count;
            s.scan_summary.note = format!("已发现候选补入 {count} 部，保留其余候选等待空位");
            s.scan_summary.more = !s.discovery_pending.is_empty() || s.discovery.more();
            s.next_scan = now() + 3;
            if count > 0 {
                s.log(
                    None,
                    format!("已发现候选补入 {count} 部；继续复用采集分页进度"),
                );
            }
            Ok(())
        });
    }
    let mut discovery = snapshot.discovery.clone();
    let fresh_cycle = discovery.prepare(&config);
    let Some(selected) = discovery.select(now()) else {
        return service.transaction(|s| {
            if s.config.as_ref() == Some(&config) {
                s.next_scan = discovery.next_at(now(), &config);
                s.discovery = discovery;
            }
            Ok(())
        });
    };
    let stream = discovery.streams[selected].clone();
    let result = api(app, stream.path()).await;
    ensure_running(service)?;
    let page = match result {
        Ok(page) if page["items"].is_array() => page,
        result => {
            let message = result
                .err()
                .map(|e| e.message)
                .unwrap_or_else(|| "列表响应不完整".into());
            discovery.streams[selected].failed(now(), &config);
            return service.transaction(|s| {
                if s.config.as_ref() != Some(&config) {
                    return Ok(());
                }
                if fresh_cycle {
                    s.scan_summary = ScanSummary::default();
                }
                s.scan_summary.source = stream.label.clone();
                s.scan_summary.page = stream.page + 1;
                s.scan_summary.device_round = if stream.kind == "recommend" {
                    stream.round + 1
                } else {
                    0
                };
                s.scan_summary.note = format!(
                    "{}暂不可用，其他来源继续；该来源稍后自动恢复：{message}",
                    stream.label
                );
                s.scan_summary.more = discovery.more();
                s.scan_summary.at = now();
                s.warning = s.scan_summary.note.clone();
                s.log(None, s.warning.clone());
                s.last_scan = now();
                s.next_scan = discovery.next_at(now(), &config);
                s.discovery = discovery;
                Ok(())
            });
        }
    };
    let mut candidates = vec![];
    let values = page["items"].as_array().unwrap();
    let checked = values.len();
    let mut metrics_failed = 0;
    for value in values {
        ensure_running(service)?;
        if let Some(mut c) = source::parse_candidate(value) {
            // Mixed recommendations must retain their real type. Never relabel
            // a live-action recommendation as animation to pass the filter.
            if c.release_type.is_empty() && !stream.release_type.is_empty() {
                c.release_type = stream.release_type.clone();
            }
            let mut date_free = config.clone();
            date_free["scope"] = json!("all");
            if !source::eligible(&c, &date_free, now() as i64) {
                continue;
            }
            if text(&config, "scope") == "today" && c.online_time.is_none() {
                match api(
                    app,
                    format!(
                        "/api/duanju/series-metrics?series_id={}&content_type={}",
                        urlencoding::encode(&c.book_id),
                        c.content_type
                    ),
                )
                .await
                {
                    Ok(metrics) => c.online_time = metrics["online_time"].as_i64(),
                    Err(_) => {
                        metrics_failed += 1;
                        continue;
                    }
                }
            }
            if source::eligible(&c, &config, now() as i64) {
                candidates.push(c);
            }
        }
    }
    ensure_running(service)?;
    let note = discovery.streams[selected].accept(&page, &config);
    service.transaction(|s| {
        if !super::pipeline::can_scan(s) || s.config.as_ref() != Some(&config) {
            return Ok(());
        }
        if fresh_cycle {
            s.scan_summary = ScanSummary::default();
        }
        let filtered = checked.saturating_sub(candidates.len());
        let known = candidates
            .iter()
            .filter(|c| {
                let id = Task::new((*c).clone(), config.clone(), save_dir.clone()).id;
                s.jobs.iter().any(|j| j.id == id)
            })
            .count();
        let count = buffer_candidates(s, candidates, &config, &save_dir);
        s.scan_summary.checked += checked;
        s.scan_summary.filtered += filtered;
        s.scan_summary.known += known;
        s.scan_summary.added += count;
        s.scan_summary.more = discovery.more() || !s.discovery_pending.is_empty();
        s.scan_summary.source = stream.label;
        s.scan_summary.page = stream.page + 1;
        s.scan_summary.device_round = if stream.kind == "recommend" {
            stream.round + 1
        } else {
            0
        };
        s.scan_summary.note = if metrics_failed > 0 {
            format!("{metrics_failed} 部上线日期查询失败，留待下轮核对。{note}")
        } else {
            note
        };
        s.scan_summary.at = now();
        s.warning.clear();
        s.last_scan = now();
        s.next_scan = if !s.discovery_pending.is_empty() {
            now() + 3
        } else {
            discovery.next_at(now(), &config)
        };
        s.discovery = discovery;
        if count > 0 {
            s.log(
                None,
                format!("本轮补入 {count} 部；每组最多 10 部，完成正片、Shorts 与清理收尾或跳过后自动补位"),
            );
        }
        Ok(())
    })
}

pub async fn advance(
    app: &AppHandle,
    service: &Arc<Service>,
    task: &mut Task,
) -> Result<(), AppError> {
    ensure_task_running(service, &task.id)?;
    // Also check merged files created by older versions before resuming AI or
    // upload. Cache only while the exact path/size/modification time still match.
    if !task.main_done
        && matches!(
            task.stage.as_str(),
            "separate" | "subtitles" | "metadata" | "upload" | "short"
        )
    {
        if let Some(path) = task.merged.clone() {
            let cached = task.duration_check.clone();
            task.duration_check = crate::run_blocking(move || {
                duration::inspect(&path, cached, |p| {
                    Ok(tools()?.probe_media(p)?.duration_seconds)
                })
            })
            .await?;
            if let Some(seconds) = task.duration_check.as_ref().map(|check| check.seconds) {
                if duration::skip(task, seconds) {
                    cancel_owned(app, task);
                    return Ok(());
                }
            }
        }
    }
    match task.stage.as_str() {
        "inspect" => inspect(app, task).await?,
        "download" => download(app, service, task).await?,
        "merge" => merge(app, service, task, false).await?,
        "separate" => ai_media(app, service, task, false).await?,
        "subtitles" => ai_media(app, service, task, true).await?,
        "metadata" => {
            let skip = task.ai_started;
            task.ai_started = true;
            service.checkpoint(task)?;
            super::metadata::prepare(app, task, skip).await?;
            if let Some(path) = task.cover.clone() {
                own(task, &path)?;
            }
            let note = task.message.clone();
            task.next("upload", &format!("文案与封面已准备；{note}"));
        }
        "upload" => upload(app, service, task, false).await?,
        "short" => short(app, service, task).await?,
        "cleanup" => {
            let mut cleaned = task.clone();
            let (cleaned, summary) = crate::run_blocking(move || {
                let summary = cleanup(&mut cleaned)?;
                Ok((cleaned, summary))
            })
            .await?;
            *task = cleaned;
            task.cleanup_version = 1;
            task.stage = "done".into();
            task.status = Status::Completed;
            task.progress = 100.0;
            task.message = if !task.short_video_url.is_empty() {
                format!("{summary}；正片与首集 Shorts 已完成")
            } else {
                summary
            };
        }
        "done" => task.status = Status::Completed,
        _ => {
            return Err(AppError::new(
                "AUTOMATION_STAGE_INVALID",
                "未知任务阶段，保留记录等待检查",
            ));
        }
    }
    Ok(())
}
// A persisted video ID proves transfer, not processing or attachment completion.
async fn duplicate_ready(
    app: &AppHandle,
    task: &mut Task,
    found: &crate::youtube::duplicates::DuplicateMatch,
) -> Result<bool, AppError> {
    let youtube = app.state::<AppState>().youtube.clone();
    let known = youtube.snapshot();
    if let Some(job) = known
        .jobs
        .iter()
        .find(|j| j.video_id.as_deref() == Some(&found.video_id))
    {
        if matches!(
            job.status,
            YouTubeJobStatus::VideoUploadedThumbnailFailed
                | YouTubeJobStatus::VideoUploadedSubtitleFailed
                | YouTubeJobStatus::Failed
                | YouTubeJobStatus::Cancelled
        ) {
            task.status = Status::Review;
            task.message =
                "已有视频的上传或附件尚未成功，请先在上传任务中补齐；保留全部文件".into();
            return Ok(false);
        }
        if job.status != YouTubeJobStatus::Completed {
            task.status = Status::Observing;
            task.retry_at = now() + 30;
            task.message = "已有视频任务仍在处理，等待完成后继续；保留全部文件".into();
            return Ok(false);
        }
    }
    if !youtube
        .automation_processing_ready(text(&task.config, "channel"), &found.video_id)
        .await?
    {
        task.status = Status::Observing;
        task.retry_at = now() + 30;
        task.message = "已发现重复视频，等待 YouTube 处理完成；保留全部文件".into();
        return Ok(false);
    }
    Ok(true)
}
async fn inspect(app: &AppHandle, task: &mut Task) -> Result<(), AppError> {
    let catalogue = api(
        app,
        format!(
            "/api/duanju/catalog?book_id={}",
            urlencoding::encode(&task.book_id)
        ),
    )
    .await?;
    if flag(&task.config, "completeOnly") {
        let detail = api(
            app,
            format!(
                "/api/duanju/detail?book_id={}",
                urlencoding::encode(&task.book_id)
            ),
        )
        .await?;
        if let Some(count) = source::episode_count_evidence(&detail) {
            task.source.episode_count = count;
        }
        let complete = source::completion_evidence(&detail)
            .or_else(|| source::completion_evidence(&catalogue))
            .or(task.source.complete);
        task.source.complete = complete;
        if complete != Some(true) {
            task.status = Status::Observing;
            task.retry_at = now() + number(&task.config, "interval", 5) * 60;
            task.message = if complete == Some(false) {
                "尚未完结，继续观察".into()
            } else {
                "源站未提供明确完结状态，继续观察；可关闭仅完结限制".into()
            };
            return Ok(());
        }
    }
    if let Some(count) = source::episode_count_evidence(&catalogue) {
        task.source.episode_count = count;
    }
    task.episodes = source::parse_catalogue(&catalogue, task.source.episode_count)?;
    task.episode_total = task.episodes.len();
    let limit = number(&task.config, "maxEpisodes", 300);
    if limit > 0 && task.episode_total as u64 > limit {
        task.status = Status::Skipped;
        task.message = format!(
            "目录确认共 {} 集，超过最多 {limit} 集设置，跳过整部剧",
            task.episode_total
        );
        return Ok(());
    }
    let query = DuplicateQuery {
        channel_id: text(&task.config, "channel").into(),
        title: task.title.chars().take(100).collect(),
        book_id: task.book_id.clone(),
        drama_title: task.title.clone(),
        season: task.season,
        upload_format: serde_json::from_value(task.config["uploadFormat"].clone())
            .unwrap_or_default(),
        source_path: None,
    };
    let matches = app.state::<AppState>().youtube.check_upload(&query).await?;
    if let Some(found) = matches
        .iter()
        .find(|m| m.confidence == MatchConfidence::Confirmed && !m.youtube_url.is_empty())
    {
        if !duplicate_ready(app, task, found).await? {
            return Ok(());
        }
        task.main_done = true;
        task.main_video_url = found.youtube_url.clone();
        task.allow_duplicate = false;
        if !flag(&task.config, "firstEpisodeShorts") {
            task.status = Status::Skipped;
            task.message = "同频道、同季、同类型已存在，跳过".into();
            return Ok(());
        }
    } else if matches
        .iter()
        .any(|m| m.confidence == MatchConfidence::Confirmed)
        || (!task.allow_duplicate && !matches.is_empty())
    {
        task.status = Status::Review;
        task.message = "频道或队列中存在相似内容；核对季数、视频类型及任务状态后继续".into();
        return Ok(());
    }
    task.next("download", "检查通过，准备下载并保留源剧名和 ID");
    Ok(())
}
async fn download(
    app: &AppHandle,
    service: &Arc<Service>,
    task: &mut Task,
) -> Result<(), AppError> {
    safe_root(&task.root)?;
    fs::create_dir_all(&task.root).map_err(|_| invalid_file())?;
    check_disk(&task.root, number(&task.config, "minDisk", 20))?;
    let marker = task.root.join("自动追剧记录.json");
    if marker.exists() {
        let old: Value = serde_json::from_slice(&fs::read(&marker).map_err(|_| invalid_file())?)
            .map_err(|_| invalid_file())?;
        if text(&old, "id") != task.id {
            return Err(invalid_file());
        }
    } else {
        crate::atomic_write(&marker,&serde_json::to_vec(&json!({"id":task.id,"title":task.title,"bookId":task.book_id,"season":task.season})).unwrap(),"任务标记")?;
    }
    let wanted = if task.main_done {
        1
    } else {
        task.episodes.len()
    };
    if task.files.len() >= wanted {
        task.next(
            if task.main_done { "metadata" } else { "merge" },
            "所需剧集已下载并校验",
        );
        return Ok(());
    }
    let episode = task.episodes[task.files.len()].clone();
    let prefix = format!(
        "{:04}_{}",
        episode.index,
        crate::sanitize_name(&episode.item_id)
    );
    let existing = fs::read_dir(&task.root)
        .map_err(|_| invalid_file())?
        .filter_map(Result::ok)
        .map(|v| v.path())
        .find(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("mp4")
                && p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&format!("{prefix}_"))
        });
    let a = app.clone();
    let root = task.root.clone();
    let source = task.source.clone();
    let config = task.config.clone();
    let id = format!("auto-{}-{}", task.id, episode.index);
    let receipt = task.root.join(format!("下载完成-{}.json", episode.index));
    let orientation = text(&task.config, "orientation").to_owned();
    ensure_task_running(service, &task.id)?;
    task.message = format!(
        "正在{}第 {}/{} 集",
        if existing.is_some() {
            "校验已有"
        } else {
            "下载"
        },
        episode.index,
        wanted
    );
    service.checkpoint(task)?;
    let owner = service.clone();
    let task_id = task.id.clone();
    let path = crate::run_blocking(move || {
        ensure_task_running(&owner, &task_id)?;
        let verify_download = |path: &Path| -> Result<(), AppError> {
            let probe = tools()?.probe_media(path).map_err(|error| {
                AppError::with_cause(
                    "AUTOMATION_MEDIA_PROBE_FAILED",
                    "下载文件媒体检测失败，已清理文件并自动重试",
                    error.message.clone(),
                )
            })?;
            if probe.duration_seconds <= 0.0 {
                return Err(AppError::new(
                    "AUTOMATION_MEDIA_PROBE_FAILED",
                    "下载文件时长无效，已清理文件并自动重试",
                ));
            }
            Ok(())
        };
        if let Some(path) = existing {
            safe_root(&path)?;
            // Older versions could leave a finished media file without the
            // sidecar receipt (or the file may have been moved and restored).
            // Re-probe the file and adopt it when it is a valid episode. This
            // keeps recovery automatic and avoids asking the user to move a
            // stale file by hand.
            match verify_download(&path) {
                Ok(()) => {
                    let actual = identity(&path)?;
                    let matches_receipt = fs::read(&receipt)
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<OwnedFile>(&bytes).ok())
                        .is_some_and(|record| {
                            record.path == path
                                && record.hash == actual.hash
                                && record.size == actual.size
                        });
                    if !matches_receipt {
                        crate::atomic_write(
                            &receipt,
                            &serde_json::to_vec(&actual).map_err(|_| invalid_file())?,
                            "补写下载完成凭据",
                        )?;
                    }
                    return Ok(path);
                }
                Err(error) if error.code == "AUTOMATION_MEDIA_PROBE_FAILED" => {
                    // It is inside this task's protected root and has failed
                    // media validation, so it is safe to remove before retry.
                    let _ = fs::remove_file(&path);
                    let _ = fs::remove_file(&receipt);
                }
                Err(error) => return Err(error),
            }
        }
        let state = a.state::<AppState>();
        crate::ensure_api_ready(&state.client, &state.api_base, &state.api_ready)?;
        let result = crate::perform_download_episode(
            a.clone(),
            state.client.clone(),
            state.api_base.clone(),
            root.parent().ok_or_else(invalid_file)?.into(),
            crate::DownloadArgs {
                task_id: id,
                item_id: episode.item_id,
                title: root
                    .file_name()
                    .ok_or_else(invalid_file)?
                    .to_string_lossy()
                    .into_owned(),
                episode_title: prefix,
                definition: Some(text(&config, "definition").into()),
                series: crate::DownloadSeriesMetadata {
                    cover_url: source.cover,
                    summary: source.summary,
                    author: String::new(),
                    category: source.category,
                    content_type_code: source.content_type,
                    release_type: Some(source.release_type),
                    episode_count: source.episode_count,
                    duration_seconds: 0,
                    online_time: source.online_time,
                },
            },
            state.prepared_series_assets.clone(),
        )?;
        let path = PathBuf::from(result.path);
        match verify_download(&path) {
            Ok(()) => {}
            Err(error) if error.code == "AUTOMATION_MEDIA_PROBE_FAILED" => {
                let _ = fs::remove_file(&path);
                return Err(error);
            }
            Err(error) => return Err(error),
        }
        let record = identity(&path)?;
        crate::atomic_write(
            &receipt,
            &serde_json::to_vec(&record).map_err(|_| invalid_file())?,
            "下载完成凭据",
        )?;
        Ok(path)
    })
    .await?;
    own(task, &path)?;
    if task.files.is_empty() && orientation != "all" {
        let p = path.clone();
        let actual =
            crate::run_blocking(move || crate::youtube::format::orientation_file(&p)).await?;
        if orientation != actual {
            task.status = Status::Skipped;
            task.message = "首集实际画幅不符合筛选条件，未下载其余剧集；保留首集".into();
            return Ok(());
        }
    }
    task.files.push(path);
    task.episode_done = task.files.len();
    task.progress = 100.0 * task.episode_done as f64 / wanted as f64;
    task.message = format!("已下载并校验 {}/{} 集", task.episode_done, wanted);
    Ok(())
}

fn reusable_media(job: &MediaJob) -> bool {
    if job.status != MediaJobStatus::Completed {
        return true;
    }
    // Missing completed outputs must not pin recovery to an obsolete history entry.
    let outputs: Vec<_> = job
        .output_path
        .iter()
        .chain(job.outputs.iter().map(|o| &o.path))
        .collect();
    !outputs.is_empty() && outputs.iter().all(|p| p.try_exists().unwrap_or(true))
}

fn current_media(
    app: &AppHandle,
    id: &Option<String>,
    book: &str,
    kind: MediaJobKind,
) -> Option<MediaJob> {
    app.state::<AppState>()
        .media_jobs
        .snapshot()
        .jobs
        .into_iter()
        .filter(reusable_media)
        .find(|j| {
            id.as_ref().is_some_and(|id| *id == j.id)
                || j.kind == kind
                    && (j.merge_request.as_ref().is_some_and(|r| r.book_id == book)
                        || j.ai_request.as_ref().is_some_and(|r| r.book_id == book))
        })
}
fn refresh_media(
    app: &AppHandle,
    task: &mut Task,
    job: MediaJob,
) -> Result<Option<MediaJob>, AppError> {
    task.media_state = match job.status {
        MediaJobStatus::Queued => Some("queued".into()),
        MediaJobStatus::Running => Some("running".into()),
        MediaJobStatus::Paused => Some("paused".into()),
        _ => None,
    };
    task.progress = job.percent;
    task.message = format!(
        "{}：{}",
        match job.kind {
            MediaJobKind::Merge => "合并",
            MediaJobKind::SeparateBackgroundMusic => "分离背景音乐",
            _ => "提取字幕",
        },
        match job.stage.as_str() {
            "queued" => "等待可用 CPU / GPU 资源",
            "probing" => "正在检测媒体参数",
            "merging" => "正在合并视频",
            "completed" => "处理和输出校验完成",
            "failed" => "处理失败",
            "paused" => "已暂停，保留当前进度",
            stage => stage,
        }
    );
    match job.status {
        MediaJobStatus::Completed => Ok(Some(job)),
        MediaJobStatus::Failed | MediaJobStatus::Cancelled => {
            if task.retry_ready {
                task.retry_ready = false;
                retry_media(app, &job)?;
                Ok(None)
            } else {
                Err(AppError::new(
                    job.error_code
                        .unwrap_or_else(|| "AUTOMATION_MEDIA_FAILED".into()),
                    job.error_message
                        .unwrap_or_else(|| "媒体处理失败，保留文件".into()),
                ))
            }
        }
        MediaJobStatus::Interrupted => {
            retry_media(app, &job)?;
            Ok(None)
        }
        MediaJobStatus::Paused => {
            app.state::<AppState>().media_jobs.resume(&job.id)?;
            Ok(None)
        }
        _ => Ok(None),
    }
}
fn retry_media(app: &AppHandle, job: &MediaJob) -> Result<MediaJob, AppError> {
    let state = app.state::<AppState>();
    if job.kind == MediaJobKind::Merge {
        state.media_jobs.retry_automation_merge(&job.id)
    } else {
        state.media_jobs.retry(&job.id)
    }
}

async fn merge(
    app: &AppHandle,
    service: &Arc<Service>,
    task: &mut Task,
    is_short: bool,
) -> Result<(), AppError> {
    let book = format!(
        "auto-{}-{}",
        task.id,
        if is_short { "short-merge" } else { "merge" }
    );
    let old = if is_short {
        task.short_merge_job.clone()
    } else {
        task.merge_job.clone()
    };
    let job = if let Some(job) = current_media(app, &old, &book, MediaJobKind::Merge) {
        job
    } else {
        ensure_task_running(service, &task.id)?;
        if !is_short {
            task.message = "正在核对合并总时长，超过 12 小时将自动跳过".into();
            service.checkpoint(task)?;
            let paths = task.files.clone();
            let owner = service.clone();
            let task_id = task.id.clone();
            let over = crate::run_blocking(move || {
                duration::inputs_over_limit(&paths, |p| {
                    ensure_task_running(&owner, &task_id)?;
                    Ok(tools()?.probe_media(p)?.duration_seconds)
                })
            })
            .await?;
            if let Some(seconds) = over {
                duration::skip(task, seconds);
                return Ok(());
            }
        }
        let root = if is_short {
            task.root.join("Shorts")
        } else {
            task.root.clone()
        };
        fs::create_dir_all(&root).map_err(|_| invalid_file())?;
        let paths = if is_short {
            let copy = root.join("首集源视频.mp4");
            let receipt = root.join("首集副本凭据.json");
            if !copy.exists() {
                let mut dest = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&copy)
                    .map_err(|_| invalid_file())?;
                let mut source = fs::File::open(task.files.first().ok_or_else(invalid_file)?)
                    .map_err(|_| invalid_file())?;
                std::io::copy(&mut source, &mut dest).map_err(|_| invalid_file())?;
                dest.sync_all().map_err(|_| invalid_file())?;
                crate::atomic_write(
                    &receipt,
                    &serde_json::to_vec(&identity(&copy)?).unwrap(),
                    "首集副本凭据",
                )?;
            } else {
                let record: OwnedFile =
                    serde_json::from_slice(&fs::read(&receipt).map_err(|_| {
                        AppError::new(
                            "AUTOMATION_LOCAL_REVIEW",
                            "首集副本无完成凭据，请移走冲突文件后继续",
                        )
                    })?)
                    .map_err(|_| invalid_file())?;
                let actual = identity(&copy)?;
                if record.path != copy || record.hash != actual.hash {
                    return Err(invalid_file());
                }
            }
            own(task, &copy)?;
            vec![copy]
        } else {
            task.files.clone()
        };
        let request = StartMergeRequest {
            book_id: book,
            title: task.title.clone(),
            series_root: root,
            output_file_name: format!("{}.mp4", crate::sanitize_name(&task.title)),
            inputs: paths
                .into_iter()
                .enumerate()
                .map(|(i, path)| StartMergeInput {
                    episode_index: i as u32 + 1,
                    path,
                })
                .collect(),
            transcode_h264: false,
            square_canvas: is_short,
            mode: Some(MergeMode::Auto),
            quality: Default::default(),
            conflict_policy: Default::default(),
        };
        let media = app.state::<AppState>().media_jobs.clone();
        let owner = service.clone();
        let task_id = task.id.clone();
        crate::run_blocking(move || {
            ensure_task_running(&owner, &task_id)?;
            media.start_merge(request)
        })
        .await?
    };
    if is_short {
        task.short_merge_job = Some(job.id.clone())
    } else {
        task.merge_job = Some(job.id.clone())
    };
    service.checkpoint(task)?;
    ensure_task_running(service, &task.id)?;
    if let Some(job) = refresh_media(app, task, job)? {
        let path = job.output_path.ok_or_else(invalid_file)?;
        own(task, &path)?;
        if is_short {
            task.short_path = Some(path)
        } else {
            task.merged = Some(path.clone());
            task.prepared = Some(path);
            task.next("separate", "合并校验完成");
        }
    }
    Ok(())
}
// Rebuild only missing derived files, from receipts that still match the originals.
// The caller runs this on the blocking pool because validation hashes every source.
fn recover_missing_media_input(task: &mut Task, input: &Path) -> Result<bool, AppError> {
    if task.main_done
        || !task.main_video_url.is_empty()
        || !matches!(task.stage.as_str(), "separate" | "subtitles")
    {
        return Ok(false);
    }
    safe_root(input)?;
    if input.try_exists().map_err(|_| invalid_file())? {
        return Ok(false);
    }
    safe_root(&task.root)?;
    let root = fs::canonicalize(&task.root).map_err(|_| invalid_file())?;
    if !input.starts_with(&root)
        || input
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(invalid_file());
    }
    let verify = |path: &Path| -> Result<(), AppError> {
        safe_root(path)?;
        let canonical = fs::canonicalize(path).map_err(|_| invalid_file())?;
        if !canonical.starts_with(&root) {
            return Err(invalid_file());
        }
        let recorded = task
            .owned
            .iter()
            .find(|owned| owned.path == canonical)
            .ok_or_else(invalid_file)?;
        let actual = identity(&canonical)?;
        if recorded.size != actual.size || recorded.hash != actual.hash {
            return Err(invalid_file());
        }
        Ok(())
    };
    let merged = task
        .merged
        .as_ref()
        .filter(|path| path.try_exists().unwrap_or(true));
    let reuse_merged = if let Some(path) = merged {
        verify(path)?;
        true
    } else {
        if task.files.is_empty()
            || task.files.len() != task.episode_done
            || task.episode_done != task.episode_total
        {
            return Err(invalid_file());
        }
        for path in &task.files {
            verify(path)?;
        }
        false
    };
    task.separate_job = None;
    task.subtitle_job = None;
    task.subtitle = None;
    task.retry_ready = false;
    if reuse_merged {
        task.prepared = task.merged.clone();
        task.next(
            if flag(&task.config, "separate") {
                "separate"
            } else {
                "subtitles"
            },
            "处理产物缺失，已校验原合并视频，重新生成媒体处理产物",
        );
    } else {
        task.merge_job = None;
        task.merged = None;
        task.prepared = None;
        task.next(
            "merge",
            "处理产物缺失，已校验全部原集文件，重建合并视频，无需重新下载",
        );
    }
    Ok(true)
}

async fn ai_media(
    app: &AppHandle,
    service: &Arc<Service>,
    task: &mut Task,
    subtitles: bool,
) -> Result<(), AppError> {
    let next = if subtitles { "metadata" } else { "subtitles" };
    if !flag(
        &task.config,
        if subtitles { "subtitles" } else { "separate" },
    ) {
        task.next(next, "按设置跳过未启用的媒体处理");
        return Ok(());
    }
    let kind = if subtitles {
        MediaJobKind::ExtractSubtitles
    } else {
        MediaJobKind::SeparateBackgroundMusic
    };
    let book = format!(
        "auto-{}-{}",
        task.id,
        if subtitles { "subtitles" } else { "separate" }
    );
    let old = if subtitles {
        task.subtitle_job.clone()
    } else {
        task.separate_job.clone()
    };
    let job = if let Some(job) = current_media(app, &old, &book, kind) {
        job
    } else {
        ensure_task_running(service, &task.id)?;
        let state = app.state::<AppState>();
        let settings = state.settings.lock().map_err(|_| invalid_file())?.clone();
        let input = if subtitles && text(&task.config, "subtitleSource") == "original" {
            task.merged.clone()
        } else {
            task.prepared.clone()
        }
        .ok_or_else(invalid_file)?;
        let mut recovered = task.clone();
        let missing_input = input.clone();
        let recovery = crate::run_blocking(move || {
            recover_missing_media_input(&mut recovered, &missing_input)
                .map(|changed| changed.then_some(recovered))
        })
        .await?;
        if let Some(recovered) = recovery {
            *task = recovered;
            service.checkpoint(task)?;
            return Ok(());
        }
        let request = StartAIJobRequest {
            book_id: book,
            title: task.title.clone(),
            series_root: task.root.clone(),
            scope: MediaJobScope::Merged,
            inputs: vec![StartMergeInput {
                episode_index: 1,
                path: input,
            }],
            model: serde_json::to_value(if subtitles {
                serde_json::to_value(settings.whisper_model).unwrap()
            } else {
                serde_json::to_value(settings.demucs_model).unwrap()
            })
            .unwrap()
            .as_str()
            .unwrap()
            .into(),
            device: settings.ai_device.as_str().into(),
        };
        let media = state.media_jobs.clone();
        let owner = service.clone();
        let task_id = task.id.clone();
        crate::run_blocking(move || {
            ensure_task_running(&owner, &task_id)?;
            media.start_ai(request, kind)
        })
        .await?
    };
    if subtitles {
        task.subtitle_job = Some(job.id.clone())
    } else {
        task.separate_job = Some(job.id.clone())
    }
    service.checkpoint(task)?;
    ensure_task_running(service, &task.id)?;
    if let Some(job) = refresh_media(app, task, job)? {
        for output in &job.outputs {
            own(task, &output.path)?;
        }
        let expected = if subtitles {
            MediaJobOutputKind::Subtitles
        } else {
            MediaJobOutputKind::NoBackgroundMusicVideo
        };
        let path = job
            .outputs
            .iter()
            .find(|o| o.kind == expected)
            .map(|o| o.path.clone())
            .ok_or_else(invalid_file)?;
        if subtitles {
            task.subtitle = Some(if text(&task.config, "subtitleFormat") == "vtt" {
                let output = path.with_extension("vtt");
                let a = path.clone();
                let b = output.clone();
                crate::run_blocking(move || {
                    let result = tools()?
                        .ffmpeg_command()
                        .args(["-v", "error", "-nostdin", "-y", "-i"])
                        .arg(a)
                        .arg(&b)
                        .output()
                        .map_err(|_| invalid_file())?;
                    if !result.status.success() {
                        return Err(invalid_file());
                    }
                    Ok(())
                })
                .await?;
                own(task, &output)?;
                output
            } else {
                path
            });
        } else {
            task.prepared = Some(path);
        }
        task.next(next, "媒体处理完成并保留输出路径");
    }
    Ok(())
}

async fn upload(
    app: &AppHandle,
    service: &Arc<Service>,
    task: &mut Task,
    is_short: bool,
) -> Result<(), AppError> {
    ensure_task_running(service, &task.id)?;
    if !is_short && task.main_done {
        task.next("short", "正片已有上传记录，继续核对首集 Shorts");
        return Ok(());
    }
    let id = task.upload_id(is_short);
    let youtube = app.state::<AppState>().youtube.clone();
    if let Some(job) = youtube.snapshot().jobs.into_iter().find(|j| j.id == id) {
        task.media_state = Some(
            if job.status == YouTubeJobStatus::Paused {
                "paused"
            } else {
                "running"
            }
            .into(),
        );
        task.progress = job.percent;
        task.message = format!(
            "{}上传：{:?}",
            if is_short { "首集 Shorts" } else { "正片" },
            job.status
        );
        match job.status {
            YouTubeJobStatus::Completed => {
                let video_id = job.video_id.as_deref().ok_or_else(invalid_file)?;
                if !youtube
                    .automation_processing_ready(text(&task.config, "channel"), video_id)
                    .await?
                {
                    task.retry_at = now() + 30;
                    task.message = "视频已上传，等待 YouTube 处理完成；保留全部文件".into();
                    return Ok(());
                }
                task.allow_duplicate = false;
                if is_short {
                    task.short_done = true;
                    task.short_video_url = job.youtube_url.unwrap_or_default();
                    task.next("cleanup", "首集 Shorts 上传处理完成");
                } else {
                    task.main_done = true;
                    task.main_video_url = job.youtube_url.unwrap_or_default();
                    task.next("short", "正片上传处理完成");
                }
            }
            YouTubeJobStatus::Paused => {
                youtube.resume_upload(&id)?;
            }
            YouTubeJobStatus::Failed | YouTubeJobStatus::Cancelled => {
                if task.retry_ready {
                    task.retry_ready = false;
                    youtube.retry_upload(&id)?;
                } else {
                    return Err(AppError::new(
                        job.error_code
                            .unwrap_or_else(|| "AUTOMATION_UPLOAD_FAILED".into()),
                        job.error_message
                            .unwrap_or_else(|| "上传失败，保留文件".into()),
                    ));
                }
            }
            YouTubeJobStatus::VideoUploadedThumbnailFailed => {
                if task.retry_ready {
                    task.retry_ready = false;
                    youtube.retry_thumbnail(&id).await?;
                } else {
                    return Err(AppError::new(
                        "AUTOMATION_THUMBNAIL_FAILED",
                        "视频已上传但封面失败；保留文件，重试只补封面",
                    ));
                }
            }
            YouTubeJobStatus::VideoUploadedSubtitleFailed => {
                if task.retry_ready {
                    task.retry_ready = false;
                    youtube.upload_subtitle(&id, None).await?;
                } else {
                    return Err(AppError::new(
                        "AUTOMATION_SUBTITLE_FAILED",
                        "视频已上传但字幕失败；保留文件，重试只补字幕",
                    ));
                }
            }
            _ => {}
        }
        return Ok(());
    }
    ensure_task_running(service, &task.id)?;
    let source = if is_short {
        task.short_path.clone()
    } else {
        task.prepared.clone()
    }
    .ok_or_else(invalid_file)?;
    let m = if is_short {
        task.short_metadata.as_ref()
    } else {
        task.metadata.as_ref()
    }
    .ok_or_else(|| AppError::new("AUTOMATION_METADATA_MISSING", "发布文案缺失"))?;
    let mut description = text(m, "description").to_owned();
    if is_short && !task.main_video_url.is_empty() {
        description = format!("{}\n\n正片：{}", description, task.main_video_url);
    }
    let request = UploadIntent {
        job_id: id,
        upload_format: if is_short {
            UploadFormat::Shorts
        } else {
            serde_json::from_value(task.config["uploadFormat"].clone()).unwrap_or_default()
        },
        dedup: Some(UploadIdentity {
            channel_id: text(&task.config, "channel").into(),
            book_id: task.book_id.clone(),
            drama_title: task.title.clone(),
            season: task.season,
            allow_duplicate: task.allow_duplicate,
        }),
        file_path: source.clone(),
        cover_path: task.cover.clone(),
        subtitle: if is_short {
            None
        } else {
            task.subtitle.as_ref().map(|p| SubtitleRequest {
                path: Some(p.clone()),
                language: "zh-Hans".into(),
            })
        },
        title: text(m, "title").chars().take(100).collect(),
        description: description.chars().take(5000).collect(),
        tags: m["tags"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        category_id: text(m, "categoryId").into(),
        privacy_status: automated_privacy(task),
        self_declared_made_for_kids: false,
        contains_synthetic_media: true,
        has_paid_product_placement: false,
        audience_confirmed: true,
        synthetic_media_confirmed: true,
        publish_confirmed: true,
    };
    // Preflight can classify the actual file now, including an existing completed main upload.
    let matches = youtube
        .check_upload(&DuplicateQuery {
            channel_id: text(&task.config, "channel").into(),
            title: request.title.clone(),
            book_id: task.book_id.clone(),
            drama_title: task.title.clone(),
            season: task.season,
            upload_format: request.upload_format,
            source_path: Some(source),
        })
        .await?;
    if !matches.is_empty() {
        if let Some(found) = matches
            .iter()
            .find(|m| m.confidence == MatchConfidence::Confirmed && !m.youtube_url.is_empty())
        {
            if !duplicate_ready(app, task, found).await? {
                return Ok(());
            }
            task.allow_duplicate = false;
            if is_short {
                task.short_done = true;
                task.short_video_url = found.youtube_url.clone();
                task.next("cleanup", "同季 Shorts 已有记录，跳过重复上传");
            } else {
                task.main_done = true;
                task.main_video_url = found.youtube_url.clone();
                task.next("short", "正片已有记录，跳过重复上传");
            }
            return Ok(());
        }
        if !task.allow_duplicate
            || matches
                .iter()
                .any(|m| m.confidence == MatchConfidence::Confirmed)
        {
            task.status = Status::Review;
            task.message = "上传前发现疑似重复或未完成的同类任务，核对后选择继续或跳过".into();
            return Ok(());
        }
    }
    ensure_task_running(service, &task.id)?;
    youtube.start_upload(request).await?;
    if service.is_skipped(&task.id) {
        let _ = youtube.cancel_upload(&task.upload_id(is_short));
        return ensure_task_running(service, &task.id);
    }
    task.allow_duplicate = false;
    task.message = "已加入真实 YouTube 上传队列，等待处理结果".into();
    Ok(())
}

fn automated_privacy(task: &Task) -> PrivacyStatus {
    let ai_cover_succeeded = task
        .cover
        .as_ref()
        .and_then(|path| path.file_stem())
        .and_then(|stem| stem.to_str())
        == Some("生成封面");
    if ai_cover_succeeded {
        PrivacyStatus::Public
    } else {
        PrivacyStatus::Unlisted
    }
}
async fn short(app: &AppHandle, service: &Arc<Service>, task: &mut Task) -> Result<(), AppError> {
    if !task.shorts_required() || task.short_done {
        task.next("cleanup", "全部启用的上传已完成");
        return Ok(());
    }
    let first = task.files.first().cloned().ok_or_else(invalid_file)?;
    let result =
        crate::run_blocking(move || crate::youtube::format::validate_square_shorts_source(&first))
            .await;
    if let Err(e) = result {
        if e.code == "UPLOAD_SHORTS_DURATION_REQUIRED" {
            task.short_done = true;
            task.next("cleanup", "首集超过 3 分钟，跳过 Shorts，不自动截断原视频");
            return Ok(());
        }
        return Err(e);
    }
    if task.short_path.is_none() {
        merge(app, service, task, true).await?;
        return Ok(());
    }
    upload(app, service, task, true).await
}
pub fn pause_owned(app: &AppHandle, task: &Task) {
    let state = app.state::<AppState>();
    for id in [
        &task.merge_job,
        &task.separate_job,
        &task.subtitle_job,
        &task.short_merge_job,
    ]
    .into_iter()
    .flatten()
    {
        let _ = state.media_jobs.pause(id);
    }
    for short in [false, true] {
        let _ = state.youtube.pause_upload(&task.upload_id(short));
    }
}

pub fn cancel_owned(app: &AppHandle, task: &Task) {
    let state = app.state::<AppState>();
    for id in [
        &task.merge_job,
        &task.separate_job,
        &task.subtitle_job,
        &task.short_merge_job,
    ]
    .into_iter()
    .flatten()
    {
        // Pausing first prevents queued work from starting. Running workers must
        // then be cancelled so a skipped drama releases its CPU/GPU allocation.
        let _ = state.media_jobs.pause(id);
        let _ = state.media_jobs.cancel(id);
    }
    for short in [false, true] {
        let _ = state.youtube.cancel_upload(&task.upload_id(short));
    }
    // Manual skip has a different retention rule from an upload failure:
    // remove this task's complete local folder, while the task record remains
    // in automation/state.json as the deduplication marker.
    let save_dir = state
        .settings
        .lock()
        .ok()
        .map(|settings| settings.save_dir.clone());
    let Some(save_dir) = save_dir else {
        return;
    };
    if safe_root(&task.root).is_err() {
        return;
    }
    let Ok(root) = fs::canonicalize(&task.root) else {
        return;
    };
    let Ok(parent) = fs::canonicalize(root.parent().unwrap_or_else(|| Path::new("."))) else {
        return;
    };
    let Ok(save_root) = fs::canonicalize(save_dir) else {
        return;
    };
    // Require the canonical task directory to be below the configured save
    // directory and retain at least the 自动追剧 parent component.
    let owned = root.starts_with(&save_root)
        && parent.starts_with(&save_root)
        && parent.file_name().is_some_and(|name| name == "自动追剧")
        && root != save_root;
    if !owned || root.is_symlink() {
        return;
    }
    let _ = fs::remove_dir_all(root);
}

#[path = "cleanup.rs"]
mod cleanup_files;

#[path = "duration.rs"]
mod duration;

fn cleanup(task: &mut Task) -> Result<String, AppError> {
    if !task.main_done || (task.shorts_required() && !task.short_done) {
        return Err(AppError::new(
            "AUTOMATION_CLEANUP_BLOCKED",
            "上传尚未完成，保留所有文件",
        ));
    }
    for file in &task.owned {
        let episode = task.files.contains(&file.path)
            || file.path.file_name().is_some_and(|n| n == "首集源视频.mp4");
        let subtitle = matches!(
            file.path.extension().and_then(|e| e.to_str()),
            Some("srt" | "vtt")
        );
        let remove = if episode {
            flag(&task.config, "deleteEpisodes")
        } else if subtitle {
            !flag(&task.config, "keepSubtitles")
        } else {
            flag(&task.config, "deleteFinal")
        };
        if !remove || !file.path.exists() {
            continue;
        }
        safe_root(&file.path)?;
        if !fs::canonicalize(&file.path)
            .map_err(|_| invalid_file())?
            .starts_with(fs::canonicalize(&task.root).map_err(|_| invalid_file())?)
        {
            return Err(invalid_file());
        }
        let actual = identity(&file.path)?;
        if actual.size != file.size || actual.hash != file.hash {
            return Err(invalid_file());
        }
        fs::remove_file(&file.path).map_err(|_| invalid_file())?;
    }
    cleanup_files::finish(task)
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
