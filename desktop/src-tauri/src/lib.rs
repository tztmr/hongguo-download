use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(debug_assertions)]
use std::process::{Child, Stdio};
use std::{
    fs,
    io::Write,
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, State};
#[cfg(not(debug_assertions))]
use tauri_plugin_shell::{process::CommandChild, ShellExt};

pub mod app_error;
pub mod media;
mod settings;
pub mod youtube;
use app_error::AppError;
use media::{
    ComponentManager, ComponentStatus, MediaJob, MediaJobEventSink, MediaJobKind, MediaJobManager,
    MediaJobService, MediaJobsSnapshot, NativeAIExecutor, NativeMergeExecutor, StartAIJobRequest,
    StartMergeRequest,
};
use settings::{load_settings, save_settings, AppSettings, UpdateSettings};
use youtube::{
    models::{AccountSummary, CredentialSummary, UploadIntent, YouTubeJob, YouTubeSnapshot},
    service::{YouTubeEventSink, YouTubeService},
};

const API_CONTRACT: &str = "hongguo-desktop-v2";
const MEDIA_JOB_PROGRESS_EVENT: &str = "media-job-progress";
const AI_COMPONENT_PROGRESS_EVENT: &str = "ai-component-progress";
const YOUTUBE_JOB_PROGRESS_EVENT: &str = "youtube-job-progress";

struct AppState {
    client: Client,
    api_base: String,
    api_child: Mutex<Option<ApiChild>>,
    settings: Mutex<AppSettings>,
    settings_path: PathBuf,
    media_jobs: std::sync::Arc<MediaJobService>,
    ai_components: Option<std::sync::Arc<ComponentManager>>,
    youtube: std::sync::Arc<YouTubeService>,
}

#[derive(Clone)]
struct TauriMediaJobEventSink {
    app: AppHandle,
}

impl MediaJobEventSink for TauriMediaJobEventSink {
    fn emit(&self, job: MediaJob) {
        let _ = self.app.emit(MEDIA_JOB_PROGRESS_EVENT, job);
    }
}

#[derive(Clone)]
struct TauriYouTubeEventSink {
    app: AppHandle,
}

impl YouTubeEventSink for TauriYouTubeEventSink {
    fn emit(&self, job: YouTubeJob) {
        let _ = self.app.emit(YOUTUBE_JOB_PROGRESS_EVENT, job);
    }
}

type AppResult<T> = Result<T, AppError>;

fn err(msg: impl Into<String>) -> AppError {
    AppError::new("APP_ERROR", msg)
}

fn api_base_for_port(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

fn reserve_api_port() -> AppResult<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| err(format!("分配本地 API 端口失败: {e}")))?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|e| err(format!("读取本地 API 端口失败: {e}")))
}

fn health_matches_contract(value: &Value) -> bool {
    value.get("status").and_then(Value::as_str) == Some("ok")
        && value.get("api_contract").and_then(Value::as_str) == Some(API_CONTRACT)
}

#[cfg(debug_assertions)]
struct DevelopmentApiCommand {
    program: &'static str,
    args: Vec<String>,
}

#[cfg(debug_assertions)]
fn development_api_command(port: u16) -> DevelopmentApiCommand {
    DevelopmentApiCommand {
        program: "python3",
        args: vec![
            "-B".into(),
            "-m".into(),
            "uvicorn".into(),
            "main:app".into(),
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            port.to_string(),
            "--log-level".into(),
            "warning".into(),
        ],
    }
}

#[cfg(any(not(debug_assertions), test))]
fn release_api_args(port: u16, data_dir: &Path) -> Vec<String> {
    vec![
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
        "--data-dir".into(),
        data_dir.to_string_lossy().into_owned(),
    ]
}

fn default_save_dir() -> PathBuf {
    dirs_fallback()
}

fn dirs_fallback() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Downloads").join("红果下载")
}

fn json_ok(value: &Value) -> AppResult<&Value> {
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let msg = value
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("接口返回失败");
        return Err(err(msg));
    }
    value.get("data").ok_or_else(|| err("接口没有 data"))
}

fn get_json(client: &Client, api_base: &str, path: &str) -> AppResult<Value> {
    let url = format!("{api_base}{path}");
    let response = client
        .get(&url)
        .send()
        .map_err(|e| err(format!("请求失败: {e}")))?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|e| err(format!("读取响应失败: {e}")))?;
    if !status.is_success() {
        if let Ok(value) = serde_json::from_str::<Value>(&body) {
            if let Some(msg) = value.get("msg").and_then(Value::as_str) {
                return Err(err(msg));
            }
        }
        return Err(err(format!("HTTP {status}: {body}")));
    }
    serde_json::from_str(&body).map_err(|e| err(format!("JSON 解析失败: {e}")))
}

fn wait_for_health(client: &Client, api_base: &str, timeout: Duration) -> AppResult<()> {
    let start = Instant::now();
    loop {
        if let Ok(response) = client.get(format!("{api_base}/health")).send() {
            if response.status().is_success() {
                if let Ok(value) = response.json::<Value>() {
                    if health_matches_contract(&value) {
                        return Ok(());
                    }
                }
            }
        }
        if start.elapsed() > timeout {
            return Err(err("本地 API 启动超时"));
        }
        thread::sleep(Duration::from_millis(250));
    }
}

enum ApiChild {
    #[cfg(debug_assertions)]
    Development(Child),
    #[cfg(not(debug_assertions))]
    Release(CommandChild),
}

#[cfg(any(not(debug_assertions), test))]
fn sidecar_spawn_error(error: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "API_SIDECAR_MISSING",
        "打包的本地 API 不可用",
        error.to_string(),
    )
}

fn spawn_api(app: &AppHandle, port: u16) -> AppResult<ApiChild> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| err(format!("应用数据目录失败: {e}")))?;
    fs::create_dir_all(&data_dir).map_err(|e| err(format!("创建应用数据目录失败: {e}")))?;

    #[cfg(debug_assertions)]
    {
        let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .ok_or_else(|| err("找不到 API 项目根目录"))?;
        let spec = development_api_command(port);
        let mut command = Command::new(spec.program);
        command
            .current_dir(project_root)
            .env("HONGGUO_DATA_DIR", data_dir)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .args(spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
            .spawn()
            .map(ApiChild::Development)
            .map_err(|e| err(format!("启动开发 API 失败: {e}")))
    }

    #[cfg(not(debug_assertions))]
    {
        let command = app
            .shell()
            .sidecar("hongguo-api")
            .map_err(sidecar_spawn_error)?
            .args(release_api_args(port, &data_dir));
        let (mut events, child) = command.spawn().map_err(sidecar_spawn_error)?;
        tauri::async_runtime::spawn(async move { while events.recv().await.is_some() {} });
        Ok(ApiChild::Release(child))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_base_uses_the_reserved_port() {
        // Production mutation caught: constructing requests against a port other than the reservation.
        assert_eq!(api_base_for_port(49152), "http://127.0.0.1:49152");
    }

    #[test]
    fn api_sidecar_health_requires_the_current_desktop_api_contract() {
        // Production mutation caught: accepting a generic status response from an incompatible API.
        assert!(health_matches_contract(&serde_json::json!({
            "status": "ok",
            "api_contract": "hongguo-desktop-v2"
        })));
        assert!(!health_matches_contract(
            &serde_json::json!({ "status": "ok" })
        ));
    }

    #[test]
    fn api_sidecar_development_command_is_explicit_and_disables_bytecode() {
        // Production mutation caught: reintroducing runtime discovery or omitting python3 -B -m uvicorn.
        let command = development_api_command(49152);

        assert_eq!(command.program, "python3");
        assert_eq!(
            command.args,
            vec![
                "-B",
                "-m",
                "uvicorn",
                "main:app",
                "--host",
                "127.0.0.1",
                "--port",
                "49152",
                "--log-level",
                "warning",
            ]
        );
    }

    #[test]
    fn api_sidecar_release_arguments_include_loopback_port_and_data_dir() {
        // Production mutation caught: omitting or reordering the sidecar's isolated runtime arguments.
        assert_eq!(
            release_api_args(49152, Path::new("/tmp/hongguo-data")),
            vec![
                "--host",
                "127.0.0.1",
                "--port",
                "49152",
                "--data-dir",
                "/tmp/hongguo-data",
            ]
        );
    }

    #[test]
    fn api_sidecar_missing_release_binary_is_rejected_with_stable_code() {
        // Production mutation caught: exposing raw spawn errors or suggesting a system-Python fallback.
        let error = sidecar_spawn_error("No such file or directory");

        assert_eq!(error.code, "API_SIDECAR_MISSING");
        assert!(!error.message.contains("pip install"));
    }

    #[test]
    fn api_sidecar_shutdown_terminates_and_reaps_child() {
        // Production mutation caught: dropping the child handle without killing and waiting for it.
        let process = Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .expect("fixture child should start");
        let pid = process.id().to_string();
        let mut child = Some(ApiChild::Development(process));

        shutdown_child(&mut child);

        assert!(child.is_none());
        assert!(!Command::new("/bin/kill")
            .args(["-0", &pid])
            .stderr(Stdio::null())
            .status()
            .expect("kill probe should run")
            .success());
    }

    #[test]
    fn blocking_work_runs_off_the_calling_thread() {
        let calling_thread = thread::current().id();
        let worker_thread =
            tauri::async_runtime::block_on(run_blocking(|| Ok(thread::current().id())))
                .expect("blocking operation should complete");

        assert_ne!(calling_thread, worker_thread);
    }

    #[test]
    fn final_download_name_uses_actual_definition() {
        let path = download_destination(Path::new("/tmp/剧名"), "第 1 集", "1080p");

        assert!(path.ends_with("第 1 集_1080p.mp4"));
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProgress {
    task_id: String,
    received: u64,
    total: Option<u64>,
    percent: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadResult {
    task_id: String,
    path: String,
    definition: String,
    bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadArgs {
    task_id: String,
    item_id: String,
    title: String,
    episode_title: String,
    definition: Option<String>,
}

fn sanitize_name(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .map(|ch| {
            if matches!(ch, '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || ch as u32 == 92 {
                ' '
            } else {
                ch
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() {
        "未命名".into()
    } else {
        cleaned
    }
}

fn download_destination(series_dir: &Path, episode_title: &str, definition: &str) -> PathBuf {
    series_dir.join(format!(
        "{}_{}.mp4",
        sanitize_name(episode_title),
        sanitize_name(definition)
    ))
}

async fn run_blocking<T, F>(operation: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> AppResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|error| err(format!("后台任务失败: {error}")))?
}

#[tauri::command]
async fn api_get(state: State<'_, AppState>, path: String) -> AppResult<Value> {
    let client = state.client.clone();
    let api_base = state.api_base.clone();
    run_blocking(move || {
        let value = get_json(&client, &api_base, &path)?;
        json_ok(&value).cloned()
    })
    .await
}

#[tauri::command]
fn get_save_dir(state: State<AppState>) -> AppResult<String> {
    Ok(state
        .settings
        .lock()
        .unwrap()
        .save_dir
        .to_string_lossy()
        .to_string())
}

fn persist_settings_patch(state: &AppState, patch: UpdateSettings) -> AppResult<AppSettings> {
    let mut guard = state.settings.lock().unwrap();
    let mut next = guard.clone();
    next.apply(patch).map_err(err)?;
    fs::create_dir_all(&next.save_dir).map_err(|e| err(format!("创建目录失败: {e}")))?;
    save_settings(&state.settings_path, &next).map_err(err)?;
    *guard = next.clone();
    Ok(next)
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> AppResult<AppSettings> {
    Ok(state.settings.lock().unwrap().clone())
}

#[tauri::command]
fn update_settings(state: State<AppState>, patch: UpdateSettings) -> AppResult<AppSettings> {
    persist_settings_patch(&state, patch)
}

#[tauri::command]
fn set_save_dir(state: State<AppState>, path: String) -> AppResult<String> {
    let settings = persist_settings_patch(
        &state,
        UpdateSettings {
            save_dir: Some(path),
            ..Default::default()
        },
    )?;
    Ok(settings.save_dir.to_string_lossy().to_string())
}

#[tauri::command]
fn choose_save_dir(app: AppHandle, state: State<AppState>) -> AppResult<String> {
    use tauri_plugin_dialog::DialogExt;
    let current = state.settings.lock().unwrap().save_dir.clone();
    let selected = app
        .dialog()
        .file()
        .set_directory(&current)
        .blocking_pick_folder();
    match selected {
        Some(folder) => {
            let path = folder
                .into_path()
                .map_err(|e| err(format!("目录解析失败: {e}")))?;
            let settings = persist_settings_patch(
                &state,
                UpdateSettings {
                    save_dir: Some(path.to_string_lossy().to_string()),
                    ..Default::default()
                },
            )?;
            Ok(settings.save_dir.to_string_lossy().to_string())
        }
        None => Ok(current.to_string_lossy().to_string()),
    }
}

#[tauri::command]
fn open_save_dir(state: State<AppState>) -> AppResult<()> {
    let dir = state.settings.lock().unwrap().save_dir.clone();
    fs::create_dir_all(&dir).map_err(|e| err(format!("创建目录失败: {e}")))?;
    Command::new("open")
        .arg(&dir)
        .spawn()
        .map_err(|e| err(format!("打开目录失败: {e}")))?;
    Ok(())
}

#[tauri::command]
fn reveal_path(path: String) -> AppResult<()> {
    Command::new("open")
        .args(["-R", &path])
        .spawn()
        .map_err(|e| err(format!("打开文件失败: {e}")))?;
    Ok(())
}

#[tauri::command]
async fn health(state: State<'_, AppState>) -> AppResult<Value> {
    let client = state.client.clone();
    let api_base = state.api_base.clone();
    run_blocking(move || {
        let response = client
            .get(format!("{api_base}/health"))
            .send()
            .map_err(|e| err(format!("健康检查失败: {e}")))?;
        response
            .json()
            .map_err(|e| err(format!("健康检查解析失败: {e}")))
    })
    .await
}

#[tauri::command]
async fn download_episode(
    app: AppHandle,
    state: State<'_, AppState>,
    mut args: DownloadArgs,
) -> AppResult<DownloadResult> {
    let client = state.client.clone();
    let api_base = state.api_base.clone();
    let settings = state.settings.lock().unwrap().clone();
    let save_dir = settings.save_dir;
    if args.definition.is_none() {
        args.definition = Some(settings.definition.as_str().to_string());
    }
    run_blocking(move || perform_download_episode(app, client, api_base, save_dir, args)).await
}

#[tauri::command]
fn get_media_jobs(state: State<AppState>) -> MediaJobsSnapshot {
    state.media_jobs.snapshot()
}

#[tauri::command]
fn start_merge_job(state: State<AppState>, request: StartMergeRequest) -> AppResult<MediaJob> {
    state.media_jobs.start_merge(request)
}

#[tauri::command]
fn start_audio_separation_job(
    state: State<AppState>,
    request: StartAIJobRequest,
) -> AppResult<MediaJob> {
    state
        .media_jobs
        .start_ai(request, MediaJobKind::SeparateBackgroundMusic)
}

#[tauri::command]
fn start_subtitle_job(state: State<AppState>, request: StartAIJobRequest) -> AppResult<MediaJob> {
    state
        .media_jobs
        .start_ai(request, MediaJobKind::ExtractSubtitles)
}

#[tauri::command]
fn cancel_media_job(state: State<AppState>, job_id: String) -> AppResult<MediaJob> {
    state.media_jobs.cancel(&job_id)
}

#[tauri::command]
fn retry_media_job(state: State<AppState>, job_id: String) -> AppResult<MediaJob> {
    state.media_jobs.retry(&job_id)
}

#[tauri::command]
fn mark_media_job_notified(
    state: State<AppState>,
    job_id: String,
    outcome: String,
) -> AppResult<MediaJob> {
    match outcome.as_str() {
        "success" => state.media_jobs.mark_notified(&job_id, true),
        "failure" => state.media_jobs.mark_notified(&job_id, false),
        _ => Err(AppError::new(
            "NOTIFICATION_OUTCOME_INVALID",
            "通知结果类型无效",
        )),
    }
}

fn ai_components(state: &AppState) -> AppResult<std::sync::Arc<ComponentManager>> {
    state
        .ai_components
        .clone()
        .ok_or_else(|| AppError::new("AI_COMPONENT_MANIFEST_INVALID", "未配置可安装的媒体组件"))
}

#[tauri::command]
fn get_ai_components(state: State<AppState>) -> AppResult<Vec<ComponentStatus>> {
    Ok(ai_components(&state)?.status())
}

#[tauri::command]
fn install_ai_component(
    app: AppHandle,
    state: State<AppState>,
    id: String,
) -> AppResult<ComponentStatus> {
    let manager = ai_components(&state)?;
    manager.install(&id, |progress| {
        let _ = app.emit(AI_COMPONENT_PROGRESS_EVENT, progress);
    })
}

#[tauri::command]
fn remove_ai_component(state: State<AppState>, id: String) -> AppResult<()> {
    ai_components(&state)?.remove(&id)
}

#[tauri::command]
fn get_youtube_snapshot(state: State<AppState>) -> YouTubeSnapshot {
    state.youtube.snapshot()
}

#[tauri::command]
fn import_youtube_oauth_config(
    state: State<AppState>,
    path: String,
) -> AppResult<CredentialSummary> {
    state.youtube.import_credential(Path::new(&path))
}

#[tauri::command]
async fn authorize_youtube(state: State<'_, AppState>) -> AppResult<AccountSummary> {
    state.youtube.authorize().await
}

#[tauri::command]
fn set_youtube_channel(state: State<AppState>, channel_id: String) -> AppResult<YouTubeSnapshot> {
    state.youtube.set_active_channel(&channel_id)
}

#[tauri::command]
async fn revoke_youtube(
    state: State<'_, AppState>,
    channel_id: String,
) -> AppResult<YouTubeSnapshot> {
    state.youtube.revoke(&channel_id).await
}

#[tauri::command]
fn remove_youtube_oauth_config(state: State<AppState>) -> AppResult<YouTubeSnapshot> {
    state.youtube.remove_credential()
}

#[tauri::command]
fn start_youtube_upload_job(
    state: State<AppState>,
    request: UploadIntent,
) -> AppResult<YouTubeJob> {
    state.youtube.start_upload(request)
}

#[tauri::command]
fn cancel_youtube_upload_job(state: State<AppState>, job_id: String) -> AppResult<()> {
    state.youtube.cancel_upload(&job_id)
}

#[tauri::command]
fn retry_youtube_upload_job(state: State<AppState>, job_id: String) -> AppResult<YouTubeJob> {
    state.youtube.retry_upload(&job_id)
}

#[tauri::command]
async fn retry_youtube_thumbnail(
    state: State<'_, AppState>,
    job_id: String,
) -> AppResult<YouTubeJob> {
    state.youtube.retry_thumbnail(&job_id).await
}

#[tauri::command]
fn mark_youtube_job_notified(
    state: State<AppState>,
    job_id: String,
    outcome: String,
) -> AppResult<YouTubeJob> {
    match outcome.as_str() {
        "success" => state.youtube.mark_notified(&job_id, true),
        "failure" => state.youtube.mark_notified(&job_id, false),
        _ => Err(AppError::new(
            "NOTIFICATION_OUTCOME_INVALID",
            "通知结果类型无效",
        )),
    }
}

fn perform_download_episode(
    app: AppHandle,
    client: Client,
    api_base: String,
    save_dir: PathBuf,
    args: DownloadArgs,
) -> AppResult<DownloadResult> {
    let definition = args.definition.unwrap_or_else(|| "auto".into());
    let series_dir = save_dir.join(sanitize_name(&args.title));
    fs::create_dir_all(&series_dir).map_err(|e| err(format!("创建目录失败: {e}")))?;
    let url = format!(
        "{}/api/duanju/download?item_id={}&definition={}",
        api_base,
        urlencoding::encode(&args.item_id),
        urlencoding::encode(&definition)
    );

    let mut response = client
        .get(&url)
        .send()
        .map_err(|e| err(format!("下载请求失败: {e}")))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if let Ok(value) = serde_json::from_str::<Value>(&body) {
            if let Some(msg) = value.get("msg").and_then(Value::as_str) {
                return Err(err(msg));
            }
        }
        return Err(err(format!("下载失败 HTTP {status}: {body}")));
    }
    let actual_definition = response
        .headers()
        .get("x-duanju-definition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or(&definition)
        .to_string();
    let dest = download_destination(&series_dir, &args.episode_title, &actual_definition);
    let total = response.content_length();
    let tmp = dest.with_extension(format!("mp4.{}.part", sanitize_name(&args.task_id)));
    let mut file = fs::File::create(&tmp).map_err(|e| err(format!("写文件失败: {e}")))?;
    let mut received = 0u64;
    let mut last_emit = Instant::now();
    loop {
        let mut buffer = [0u8; 64 * 1024];
        let n = {
            use std::io::Read;
            response
                .read(&mut buffer)
                .map_err(|e| err(format!("读取视频流失败: {e}")))?
        };
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n])
            .map_err(|e| err(format!("写文件失败: {e}")))?;
        received += n as u64;
        if last_emit.elapsed() >= Duration::from_millis(120) {
            let percent = match total {
                Some(t) if t > 0 => (received as f64 / t as f64) * 100.0,
                _ => 0.0,
            };
            let _ = app.emit(
                "download-progress",
                DownloadProgress {
                    task_id: args.task_id.clone(),
                    received,
                    total,
                    percent,
                },
            );
            last_emit = Instant::now();
        }
    }
    file.flush().ok();
    drop(file);
    if dest.exists() {
        let _ = fs::remove_file(&dest);
    }
    fs::rename(&tmp, &dest).map_err(|e| err(format!("保存文件失败: {e}")))?;
    let _ = app.emit(
        "download-progress",
        DownloadProgress {
            task_id: args.task_id.clone(),
            received,
            total: Some(received),
            percent: 100.0,
        },
    );
    Ok(DownloadResult {
        task_id: args.task_id,
        path: dest.to_string_lossy().to_string(),
        definition: actual_definition,
        bytes: received,
    })
}

fn shutdown_child(child: &mut Option<ApiChild>) {
    if let Some(process) = child.take() {
        match process {
            #[cfg(debug_assertions)]
            ApiChild::Development(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            #[cfg(not(debug_assertions))]
            ApiChild::Release(child) => {
                let _ = child.kill();
            }
        }
    }
}

fn load_ai_component_manager(
    app: &AppHandle,
    data_dir: &Path,
) -> Option<std::sync::Arc<ComponentManager>> {
    let resource_dir = app.path().resource_dir().ok()?;
    let manifest_path = resource_dir.join("resources/ai-components.json");
    let json = fs::read_to_string(manifest_path).ok()?;
    let root = data_dir.join("components");
    ComponentManager::from_manifest_json(root, &json)
        .ok()
        .map(std::sync::Arc::new)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let config_dir = app
                .path()
                .app_config_dir()
                .map_err(|e| format!("应用配置目录失败: {e}"))?;
            let settings_path = config_dir.join("settings.json");
            let settings = load_settings(&settings_path, default_save_dir());
            fs::create_dir_all(&settings.save_dir).ok();
            let media_jobs_path = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("应用数据目录失败: {e}"))?;
            let media_job_manager = std::sync::Arc::new(
                MediaJobManager::load(&media_jobs_path).map_err(|e| e.to_string())?,
            );
            let client = Client::builder()
                .timeout(Duration::from_secs(180))
                .build()
                .map_err(|e| format!("http client: {e}"))?;
            let port = reserve_api_port().map_err(|e| e.to_string())?;
            let api_base = api_base_for_port(port);
            let mut child = Some(spawn_api(app.handle(), port).map_err(|e| e.to_string())?);
            if let Err(error) = wait_for_health(&client, &api_base, Duration::from_secs(20)) {
                shutdown_child(&mut child);
                return Err(error.to_string().into());
            }
            let ai_components = load_ai_component_manager(app.handle(), &media_jobs_path);
            let event_sink = std::sync::Arc::new(TauriMediaJobEventSink {
                app: app.handle().clone(),
            });
            let merge_executor = std::sync::Arc::new(NativeMergeExecutor::from_packaged_tools());
            let media_jobs = std::sync::Arc::new(if let Some(components) = ai_components.clone() {
                MediaJobService::new_with_ai(
                    media_job_manager,
                    merge_executor,
                    std::sync::Arc::new(NativeAIExecutor::from_packaged_tools(components)),
                    event_sink,
                )
            } else {
                MediaJobService::new(media_job_manager, merge_executor, event_sink)
            });
            let youtube = YouTubeService::load(
                config_dir,
                media_jobs_path,
                media_jobs.clone(),
                std::sync::Arc::new(TauriYouTubeEventSink {
                    app: app.handle().clone(),
                }),
            )
            .map_err(|error| error.to_string())?;
            app.manage(AppState {
                client,
                api_base,
                api_child: Mutex::new(child),
                settings: Mutex::new(settings),
                settings_path,
                media_jobs,
                ai_components,
                youtube,
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                if window.label() == "main" {
                    if let Some(state) = window.try_state::<AppState>() {
                        shutdown_child(&mut state.api_child.lock().unwrap());
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            api_get,
            get_settings,
            update_settings,
            get_save_dir,
            set_save_dir,
            choose_save_dir,
            open_save_dir,
            reveal_path,
            health,
            download_episode,
            get_media_jobs,
            start_merge_job,
            start_audio_separation_job,
            start_subtitle_job,
            cancel_media_job,
            retry_media_job,
            mark_media_job_notified,
            get_ai_components,
            install_ai_component,
            remove_ai_component,
            get_youtube_snapshot,
            import_youtube_oauth_config,
            authorize_youtube,
            set_youtube_channel,
            revoke_youtube,
            remove_youtube_oauth_config,
            start_youtube_upload_job,
            cancel_youtube_upload_job,
            retry_youtube_upload_job,
            retry_youtube_thumbnail,
            mark_youtube_job_notified
        ])
        .run(tauri::generate_context!())
        .expect("tauri-run-failed");
}
