use super::{
    AIExecutionResult, AIExecutor, CancellationToken, ComponentManager, MediaJobKind,
    MediaJobOutput, MediaJobOutputKind, MediaTools, MergeProgress, ValidatedAIJobRequest,
};
use crate::AppError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};

static AI_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[path = "ai_results.rs"]
mod results;
use results::{ResultStore, MUSIC, SUBTITLES, VIDEO, VOCALS};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SubtitleSource {
    EmbeddedText,
    WhisperOriginalAudio,
    WhisperVocals,
}

pub fn select_subtitle_source(codecs: &[String], has_compatible_vocals: bool) -> SubtitleSource {
    const TEXT_CODECS: [&str; 5] = ["subrip", "ass", "ssa", "webvtt", "mov_text"];
    if codecs
        .iter()
        .any(|codec| TEXT_CODECS.contains(&codec.as_str()))
    {
        SubtitleSource::EmbeddedText
    } else if has_compatible_vocals {
        SubtitleSource::WhisperVocals
    } else {
        SubtitleSource::WhisperOriginalAudio
    }
}

pub struct NativeAIExecutor {
    tools: Result<MediaTools, AppError>,
    components: Arc<ComponentManager>,
}

impl NativeAIExecutor {
    pub fn from_packaged_tools(components: Arc<ComponentManager>) -> Self {
        let tools = std::env::current_exe()
            .map_err(|error| {
                AppError::with_cause(
                    "MEDIA_TOOL_MISSING",
                    "无法定位打包的媒体工具",
                    error.to_string(),
                )
            })
            .and_then(|executable| {
                executable
                    .parent()
                    .ok_or_else(|| AppError::new("MEDIA_TOOL_MISSING", "无法定位打包的媒体工具"))
                    .and_then(MediaTools::from_resource_root)
            });
        Self { tools, components }
    }
}

fn runtime_candidates_for(platform: &str, device: &str) -> Vec<&'static str> {
    if platform != "windows" {
        return vec!["runtime"];
    }
    match device {
        "cpu" => vec!["runtime-cpu", "runtime-modern", "runtime-legacy"],
        "cuda" => vec!["runtime-modern", "runtime-legacy", "runtime"],
        _ => vec!["runtime-modern", "runtime-legacy", "runtime-cpu"],
    }
}

struct ComponentUseGuard {
    manager: Arc<ComponentManager>,
    ids: Vec<String>,
}

impl ComponentUseGuard {
    fn new(manager: Arc<ComponentManager>, ids: Vec<String>) -> Self {
        for id in &ids {
            manager.mark_in_use(id, true);
        }
        Self { manager, ids }
    }
}

impl Drop for ComponentUseGuard {
    fn drop(&mut self) {
        for id in &self.ids {
            self.manager.mark_in_use(id, false);
        }
    }
}

impl AIExecutor for NativeAIExecutor {
    fn scheduling_device(&self, request: &ValidatedAIJobRequest) -> String {
        let selected = runtime_candidates_for(std::env::consts::OS, &request.device)
            .into_iter()
            .find(|id| self.components.resolve_installed(id).is_ok());
        if request.device == "auto" && selected == Some("runtime-cpu") {
            "cpu".into()
        } else {
            request.device.clone()
        }
    }

    fn execute(
        &self,
        request: ValidatedAIJobRequest,
        cancellation: &CancellationToken,
        progress: &mut dyn FnMut(MergeProgress),
    ) -> Result<AIExecutionResult, AppError> {
        self.execute_with_threads(request, cancellation, progress, None)
    }

    fn execute_with_budget(
        &self,
        request: ValidatedAIJobRequest,
        cancellation: &CancellationToken,
        progress: &mut dyn FnMut(MergeProgress),
        budget: super::scheduling::ExecutionBudget,
    ) -> Result<AIExecutionResult, AppError> {
        let threads =
            super::scheduling::is_parallel_kind(request.kind).then_some(budget.cpu_threads);
        self.execute_with_threads(request, cancellation, progress, threads)
    }
}

impl NativeAIExecutor {
    fn execute_with_threads(
        &self,
        request: ValidatedAIJobRequest,
        cancellation: &CancellationToken,
        progress: &mut dyn FnMut(MergeProgress),
        cpu_threads: Option<usize>,
    ) -> Result<AIExecutionResult, AppError> {
        let tools = self.tools.as_ref().map_err(Clone::clone)?;
        let model_id = match request.kind {
            MediaJobKind::SeparateBackgroundMusic => format!("demucs-{}", request.model),
            MediaJobKind::ExtractSubtitles => format!("whisper-{}", request.model),
            MediaJobKind::Merge => {
                return Err(AppError::new("AI_REQUEST_INVALID", "AI 任务类型无效"))
            }
        };
        let (runtime_id, runtime) = runtime_candidates_for(std::env::consts::OS, &request.device)
            .into_iter()
            .find_map(|id| {
                self.components
                    .resolve_installed(id)
                    .ok()
                    .map(|item| (id, item))
            })
            .ok_or_else(|| {
                AppError::new(
                    "AI_COMPONENT_NOT_INSTALLED",
                    "适合当前计算设备的 AI 运行环境尚未安装",
                )
            })?;
        let model = self.components.resolve_installed(&model_id)?;
        let _in_use =
            ComponentUseGuard::new(self.components.clone(), vec![runtime_id.into(), model_id]);
        let mut outputs = Vec::new();
        let total = request.inputs.len().max(1) as f64;
        let context = AIItemContext {
            tools,
            runtime: &runtime.entrypoint,
            model_root: &model.root,
            request: &request,
            cancellation,
            cpu_threads,
        };
        for (position, input) in request.inputs.iter().enumerate() {
            if cancellation.is_cancelled() {
                cancellation.kill();
                return Err(AppError::new("AI_CANCELLED", "AI 媒体任务已取消"));
            }
            let start = position as f64 / total * 100.0;
            let span = 100.0 / total;
            let mut emit = |stage: String, percent: f64| {
                progress(MergeProgress {
                    stage: format!("第 {} 集 · {stage}", input.episode_index),
                    percent: (start + span * percent.clamp(0.0, 100.0) / 100.0).min(99.0),
                    terminal: false,
                })
            };
            let mut item_outputs = match request.kind {
                MediaJobKind::SeparateBackgroundMusic => {
                    process_separation(&context, input.episode_index, &input.path, &mut emit)?
                }
                MediaJobKind::ExtractSubtitles => {
                    process_subtitles(&context, input.episode_index, &input.path, &mut emit)?
                }
                MediaJobKind::Merge => unreachable!(),
            };
            outputs.append(&mut item_outputs);
        }
        let output_path = outputs
            .first()
            .map(|output| output.path.clone())
            .ok_or_else(|| AppError::new("AI_OUTPUT_INVALID", "AI 处理未产生可用输出"))?;
        Ok(AIExecutionResult {
            output_path,
            outputs,
        })
    }
}

struct AIItemContext<'a> {
    tools: &'a MediaTools,
    runtime: &'a Path,
    model_root: &'a Path,
    request: &'a ValidatedAIJobRequest,
    cancellation: &'a CancellationToken,
    cpu_threads: Option<usize>,
}

fn process_separation(
    context: &AIItemContext<'_>,
    episode_index: u32,
    input: &Path,
    progress: &mut dyn FnMut(String, f64),
) -> Result<Vec<MediaJobOutput>, AppError> {
    let request = context.request;
    let store = ResultStore::new(
        request,
        episode_index,
        input,
        context.runtime,
        context.model_root,
    )?;
    if let Some(result) = store.lookup() {
        return Ok(separation_outputs(
            episode_index,
            result.path(VOCALS),
            result.path(MUSIC),
            result.path(VIDEO),
        ));
    }
    let temp = JobTemp::create(&request.series_root)?;
    let wav = temp.root.join("input.wav");
    extract_audio(context.tools, input, &wav, context.cancellation)?;
    progress("separating".into(), 10.0);
    let worker_outputs = run_worker(
        WorkerInvocation {
            runtime: context.runtime,
            args: &[],
            ffmpeg: Some(context.tools.ffmpeg()),
            job_id: &request.dedupe_key,
            operation: "separate",
            input: &wav,
            output: &temp.output,
            options: json!({"model": request.model, "device": request.device, "modelRoot": context.model_root, "cpuThreads": context.cpu_threads}),
        },
        context.cancellation,
        &mut |stage, percent| progress(stage, 10.0 + percent.clamp(0.0, 100.0) * 0.7),
    )?;
    let vocals = worker_output_path(&worker_outputs, "vocalsPath", &temp.output)?;
    let music = worker_output_path(&worker_outputs, "backgroundMusicPath", &temp.output)?;
    validate_audio_file(context.tools, &vocals)?;
    validate_audio_file(context.tools, &music)?;
    progress("正在生成去背景音乐视频".into(), 85.0);
    let remuxed = temp.root.join("no-background-music.mp4");
    remux_vocals(
        context.tools,
        input,
        &vocals,
        &remuxed,
        context.cancellation,
    )?;
    context.tools.probe_media(&remuxed)?;
    progress("正在保存分离结果".into(), 95.0);
    let result = store.publish(
        &temp,
        &[(VOCALS, &vocals), (MUSIC, &music), (VIDEO, &remuxed)],
        None,
        context.cancellation,
    )?;
    Ok(separation_outputs(
        episode_index,
        result.path(VOCALS),
        result.path(MUSIC),
        result.path(VIDEO),
    ))
}

fn separation_outputs(
    episode_index: u32,
    vocals: PathBuf,
    music: PathBuf,
    video: PathBuf,
) -> Vec<MediaJobOutput> {
    vec![
        MediaJobOutput {
            episode_index,
            kind: MediaJobOutputKind::Vocals,
            path: vocals,
            source: None,
        },
        MediaJobOutput {
            episode_index,
            kind: MediaJobOutputKind::BackgroundMusic,
            path: music,
            source: None,
        },
        MediaJobOutput {
            episode_index,
            kind: MediaJobOutputKind::NoBackgroundMusicVideo,
            path: video,
            source: None,
        },
    ]
}

fn process_subtitles(
    context: &AIItemContext<'_>,
    episode_index: u32,
    input: &Path,
    progress: &mut dyn FnMut(String, f64),
) -> Result<Vec<MediaJobOutput>, AppError> {
    let request = context.request;
    let store = ResultStore::new(
        request,
        episode_index,
        input,
        context.runtime,
        context.model_root,
    )?;
    if let Some(result) = store.lookup() {
        return Ok(vec![MediaJobOutput {
            episode_index,
            kind: MediaJobOutputKind::Subtitles,
            path: result.path(SUBTITLES),
            source: result.subtitle_source,
        }]);
    }
    let temp = JobTemp::create(&request.series_root)?;
    let tracks = subtitle_tracks(context.tools, input)?;
    let vocals = store.find_vocals();
    let source = select_subtitle_source(
        &tracks
            .iter()
            .map(|(_, codec)| codec.clone())
            .collect::<Vec<_>>(),
        vocals.is_some(),
    );
    let temporary_srt = temp.root.join("subtitles.srt");
    match source {
        SubtitleSource::EmbeddedText => {
            let ordinal = tracks
                .iter()
                .find(|(_, codec)| {
                    ["subrip", "ass", "ssa", "webvtt", "mov_text"].contains(&codec.as_str())
                })
                .map(|(ordinal, _)| *ordinal)
                .ok_or_else(|| AppError::new("SUBTITLE_INVALID", "字幕轨选择失败"))?;
            export_text_subtitle(
                context.tools,
                input,
                ordinal,
                &temporary_srt,
                context.cancellation,
            )?;
            progress("exportingSubtitle".into(), 90.0);
        }
        SubtitleSource::WhisperOriginalAudio | SubtitleSource::WhisperVocals => {
            let wav = temp.root.join("input.wav");
            if let Some(vocals) = vocals.filter(|_| source == SubtitleSource::WhisperVocals) {
                fs::copy(vocals, &wav).map_err(ai_io)?;
            } else {
                extract_audio(context.tools, input, &wav, context.cancellation)?;
            }
            let outputs = run_worker(
                WorkerInvocation {
                    runtime: context.runtime,
                    args: &[],
                    ffmpeg: Some(context.tools.ffmpeg()),
                    job_id: &request.dedupe_key,
                    operation: "transcribe",
                    input: &wav,
                    output: &temp.output,
                    options: json!({"model": request.model, "device": request.device, "modelRoot": context.model_root, "cpuThreads": context.cpu_threads}),
                },
                context.cancellation,
                progress,
            )?;
            let worker_srt = worker_output_path(&outputs, "srtPath", &temp.output)?;
            fs::copy(worker_srt, &temporary_srt).map_err(ai_io)?;
        }
    }
    validate_srt(&temporary_srt)?;
    let result = store.publish(
        &temp,
        &[(SUBTITLES, &temporary_srt)],
        Some(
            match source {
                SubtitleSource::EmbeddedText => "embeddedText",
                SubtitleSource::WhisperOriginalAudio => "whisperOriginalAudio",
                SubtitleSource::WhisperVocals => "whisperVocals",
            }
            .into(),
        ),
        context.cancellation,
    )?;
    Ok(vec![MediaJobOutput {
        episode_index,
        kind: MediaJobOutputKind::Subtitles,
        path: result.path(SUBTITLES),
        source: result.subtitle_source,
    }])
}

fn safe_output_directory(root: &Path, name: &str) -> Result<PathBuf, AppError> {
    let canonical_root = fs::canonicalize(root).map_err(ai_io)?;
    let path = canonical_root.join(name);
    match fs::create_dir(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(ai_io(error)),
    }
    let metadata = fs::symlink_metadata(&path).map_err(ai_io)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AppError::new("AI_OUTPUT_INVALID", "AI 输出目录不安全"));
    }
    let canonical = fs::canonicalize(&path).map_err(ai_io)?;
    if !canonical.starts_with(canonical_root) {
        return Err(AppError::new("AI_OUTPUT_INVALID", "AI 输出目录越界"));
    }
    Ok(canonical)
}

struct JobTemp {
    root: PathBuf,
    output: PathBuf,
}

impl JobTemp {
    fn create(series_root: &Path) -> Result<Self, AppError> {
        let parent = safe_output_directory(series_root, ".hongguo-ai-work")?;
        let sequence = AI_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = parent.join(format!(
            "{}-{sequence}-{}",
            std::process::id(),
            results::nonce()?
        ));
        fs::create_dir(&root).map_err(ai_io)?;
        let output = root.join("output");
        fs::create_dir(&output).map_err(ai_io)?;
        Ok(Self { root, output })
    }
}

impl Drop for JobTemp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn extract_audio(
    tools: &MediaTools,
    input: &Path,
    output: &Path,
    cancellation: &CancellationToken,
) -> Result<(), AppError> {
    let mut command = tools.ffmpeg_command();
    command
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(input)
        .args([
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-af",
            "aresample=async=1:first_pts=0",
            "-c:a",
            "pcm_s16le",
        ])
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = run_controlled_status(
        &mut command,
        cancellation,
        "FFMPEG_FAILED",
        "音频预处理失败",
    )?;
    if !status.success() || !regular_nonempty(output) {
        return Err(AppError::new("FFMPEG_FAILED", "音频预处理失败"));
    }
    Ok(())
}

fn remux_vocals(
    tools: &MediaTools,
    video: &Path,
    vocals: &Path,
    output: &Path,
    cancellation: &CancellationToken,
) -> Result<(), AppError> {
    let mut command = tools.ffmpeg_command();
    command
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(video)
        .arg("-i")
        .arg(vocals)
        .args([
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = run_controlled_status(
        &mut command,
        cancellation,
        "FFMPEG_FAILED",
        "去背景音乐视频生成失败",
    )?;
    if !status.success() {
        return Err(AppError::new("FFMPEG_FAILED", "去背景音乐视频生成失败"));
    }
    Ok(())
}

fn validate_audio_file(tools: &MediaTools, path: &Path) -> Result<(), AppError> {
    if !regular_nonempty(path) {
        return Err(AppError::new("AI_OUTPUT_INVALID", "AI 音频输出无效"));
    }
    let output = tools
        .ffprobe_command()
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_type",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            AppError::with_cause("FFPROBE_FAILED", "AI 音频输出无法验证", error.to_string())
        })?;
    if !output.status.success()
        || serde_json::from_slice::<Value>(&output.stdout)
            .ok()
            .and_then(|value| value.get("streams").and_then(Value::as_array).cloned())
            .is_none_or(|streams| streams.is_empty())
    {
        return Err(AppError::new("AI_OUTPUT_INVALID", "AI 音频输出无效"));
    }
    Ok(())
}

fn subtitle_tracks(tools: &MediaTools, input: &Path) -> Result<Vec<(usize, String)>, AppError> {
    let output = tools
        .ffprobe_command()
        .args([
            "-v",
            "error",
            "-select_streams",
            "s",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "json",
        ])
        .arg(input)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            AppError::with_cause("FFPROBE_FAILED", "无法检查字幕轨", error.to_string())
        })?;
    if !output.status.success() {
        return Err(AppError::new("FFPROBE_FAILED", "无法检查字幕轨"));
    }
    let value: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        AppError::with_cause("FFPROBE_INVALID", "字幕轨检测结果无效", error.to_string())
    })?;
    Ok(value
        .get("streams")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(ordinal, stream)| {
            stream
                .get("codec_name")
                .and_then(Value::as_str)
                .map(|codec| (ordinal, codec.to_string()))
        })
        .collect())
}

fn export_text_subtitle(
    tools: &MediaTools,
    input: &Path,
    ordinal: usize,
    output: &Path,
    cancellation: &CancellationToken,
) -> Result<(), AppError> {
    let mut command = tools.ffmpeg_command();
    command
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(input)
        .args(["-map", &format!("0:s:{ordinal}"), "-c:s", "srt"])
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = run_controlled_status(
        &mut command,
        cancellation,
        "SUBTITLE_EXPORT_FAILED",
        "文本字幕导出失败",
    )?;
    if !status.success() {
        return Err(AppError::new("SUBTITLE_EXPORT_FAILED", "文本字幕导出失败"));
    }
    Ok(())
}

fn run_controlled_status(
    command: &mut Command,
    cancellation: &CancellationToken,
    code: &'static str,
    message: &'static str,
) -> Result<std::process::ExitStatus, AppError> {
    cancellation.prepare_command(command);
    let mut child = command
        .spawn()
        .map_err(|error| AppError::with_cause(code, message, error.to_string()))?;
    cancellation.register_child(&mut child)?;
    let child_id = child.id();
    loop {
        if cancellation.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            cancellation.clear_child(child_id);
            return Err(AppError::new("AI_CANCELLED", "AI 媒体任务已取消"));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                cancellation.clear_child(child_id);
                return Ok(status);
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                cancellation.clear_child(child_id);
                return Err(AppError::with_cause(code, message, error.to_string()));
            }
        }
    }
}

fn validate_srt(path: &Path) -> Result<(), AppError> {
    let text = fs::read_to_string(path).map_err(|error| {
        AppError::with_cause("SUBTITLE_INVALID", "SRT 字幕无效", error.to_string())
    })?;
    let mut previous = 0u64;
    let mut count = 0usize;
    for line in text.lines().filter(|line| line.contains(" --> ")) {
        let Some((start, end)) = line.split_once(" --> ") else {
            return Err(AppError::new("SUBTITLE_INVALID", "SRT 字幕时间轴无效"));
        };
        let start = parse_srt_time(start)?;
        let end = parse_srt_time(end)?;
        if start < previous || end <= start {
            return Err(AppError::new("SUBTITLE_INVALID", "SRT 字幕时间轴无效"));
        }
        previous = end;
        count += 1;
    }
    if count == 0 {
        return Err(AppError::new("SUBTITLE_NO_SPEECH", "未识别到可用对白"));
    }
    Ok(())
}

fn parse_srt_time(value: &str) -> Result<u64, AppError> {
    let parts = value
        .trim()
        .split([':', ','])
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AppError::new("SUBTITLE_INVALID", "SRT 字幕时间轴无效"))?;
    if parts.len() != 4 || parts[1] >= 60 || parts[2] >= 60 || parts[3] >= 1000 {
        return Err(AppError::new("SUBTITLE_INVALID", "SRT 字幕时间轴无效"));
    }
    Ok(((parts[0] * 60 * 60 + parts[1] * 60 + parts[2]) * 1000) + parts[3])
}

struct WorkerInvocation<'a> {
    runtime: &'a Path,
    args: &'a [&'a str],
    ffmpeg: Option<&'a Path>,
    job_id: &'a str,
    operation: &'a str,
    input: &'a Path,
    output: &'a Path,
    options: Value,
}

fn run_worker(
    invocation: WorkerInvocation<'_>,
    cancellation: &CancellationToken,
    progress: &mut dyn FnMut(String, f64),
) -> Result<Value, AppError> {
    let mut options = invocation.options;
    let cpu_threads = options
        .as_object_mut()
        .and_then(|options| options.remove("cpuThreads"))
        .and_then(|value| value.as_u64())
        .map(|threads| {
            if cfg!(windows) {
                std::thread::available_parallelism()
                    .map_or(1, usize::from)
                    .clamp(1, 16) as u64
            } else {
                threads.clamp(1, 4)
            }
        });
    if invocation.operation == "transcribe" && cfg!(target_os = "macos") {
        // The installed Torch 2.5.1 runtime cannot move Whisper's sparse alignment
        // buffers to MPS. This also fixes older installed workers without downloading
        // a replacement runtime or any model weights.
        options
            .as_object_mut()
            .ok_or_else(|| AppError::new("AI_REQUEST_INVALID", "AI 请求选项无效"))?
            .insert("device".into(), json!("cpu"));
    }
    let request = json!({
        "version": 1,
        "jobId": invocation.job_id,
        "operation": invocation.operation,
        "inputPath": invocation.input,
        "outputDir": invocation.output,
        "options": options,
    });
    let mut command = super::tools::background_command(invocation.runtime);
    if let Some(threads) = cpu_threads {
        // Torch and BLAS read these before initialization. This works with the
        // existing frozen v3 worker and also bounds accelerator-to-CPU fallback.
        for name in [
            "OMP_NUM_THREADS",
            "MKL_NUM_THREADS",
            "OPENBLAS_NUM_THREADS",
            "VECLIB_MAXIMUM_THREADS",
            "NUMEXPR_NUM_THREADS",
        ] {
            command.env(name, threads.to_string());
        }
    }
    if invocation.ffmpeg.is_some() || invocation.operation == "transcribe" {
        // Whisper launches `ffmpeg` by name. Finder-launched applications do not
        // inherit a shell PATH; use only the verified bundle and system tools.
        let ffmpeg_dir = invocation
            .ffmpeg
            .and_then(Path::parent)
            .ok_or_else(|| AppError::new("MEDIA_TOOL_MISSING", "语音转写缺少打包的 FFmpeg"))?;
        // Windows PATH lookup does not consistently accept canonical \\?\ paths.
        // Keep canonical paths for filesystem validation, simplify only for PATH.
        let mut search_paths = vec![dunce::simplified(ffmpeg_dir).to_path_buf()];
        if let Some(existing) = std::env::var_os("PATH") {
            search_paths.extend(std::env::split_paths(&existing));
        }
        let path = std::env::join_paths(search_paths).map_err(|error| {
            AppError::with_cause(
                "MEDIA_TOOL_UNSAFE",
                "语音转写媒体工具路径无效",
                error.to_string(),
            )
        })?;
        command.env("PATH", path);
    }

    command
        .args(invocation.args)
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUNBUFFERED", "1")
        .env_remove("PYTHONLEGACYWINDOWSSTDIO")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cancellation.prepare_command(&mut command);
    let mut child = command.spawn().map_err(|error| {
        AppError::with_cause(
            "AI_WORKER_FAILED",
            "无法启动 AI 工作程序",
            error.to_string(),
        )
    })?;
    if let Err(error) = cancellation.register_child(&mut child) {
        let _ = child.kill();
        let _ = child.wait();
        cancellation.clear_child(child.id());
        return Err(error);
    }
    let child_id = child.id();
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    let stderr_tail = diagnostics.clone();
    let mut stderr = child.stderr.take().expect("piped stderr");
    let stderr_reader = thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match stderr.read(&mut buffer) {
                Ok(0) => break,
                Ok(size) => {
                    let mut tail = stderr_tail.lock().unwrap();
                    tail.extend_from_slice(&buffer[..size]);
                    let discard = tail.len().saturating_sub(8192);
                    tail.drain(..discard);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });
    // Drain stdout while the process runs. Waiting for exit first both hides
    // progress and deadlocks once the worker fills the OS pipe buffer.
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::new("AI_WORKER_FAILED", "AI 工作程序输出不可用"))?;
    let (sender, receiver) = mpsc::sync_channel(32);
    let stdout_reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) => break,
                Ok(_) => {
                    if let Some(line) = decode_worker_stdout_line(&buffer) {
                        if sender.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ = sender.send(Err(error));
                    break;
                }
            }
        }
    });
    let outcome = (|| {
        if let Some(mut stdin) = child.stdin.take() {
            // Frozen older workers may ignore PYTHONUTF8 and read CP936. ASCII
            // JSON preserves every path (including emoji via surrogate pairs).
            stdin
                .write_all(ascii_worker_request(&request).as_bytes())
                .map_err(|error| {
                    AppError::with_cause("AI_WORKER_FAILED", "无法发送 AI 请求", error.to_string())
                })?;
        }
        let mut result = None;
        loop {
            if cancellation.is_cancelled() {
                return Err(AppError::new("AI_CANCELLED", "AI 媒体任务已取消"));
            }
            let line = match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => line.map_err(|error| {
                    AppError::with_cause(
                        "AI_WORKER_FAILED",
                        "无法读取 AI 工作程序输出",
                        error.to_string(),
                    )
                })?,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            // Older Whisper workers print one language-detection diagnostic even
            // with verbose=false. Accept only this known line; all other stdout
            // must remain valid protocol JSON.
            if invocation.operation == "transcribe"
                && line
                    .strip_prefix("Detected language: ")
                    .is_some_and(|language| {
                        !language.is_empty()
                            && language.len() <= 64
                            && language
                                .chars()
                                .all(|c| c.is_ascii_alphabetic() || c == ' ' || c == '-')
                    })
            {
                continue;
            }
            let event: Value = serde_json::from_str(&line).map_err(|_| {
                AppError::new("AI_WORKER_PROTOCOL_INVALID", "AI 工作程序输出格式无效")
            })?;
            match event.get("type").and_then(Value::as_str) {
                Some("progress") => progress(
                    event
                        .get("stage")
                        .and_then(Value::as_str)
                        .unwrap_or("processing")
                        .into(),
                    event.get("percent").and_then(Value::as_f64).unwrap_or(0.0),
                ),
                Some("result") => result = event.get("outputs").cloned(),
                Some("error") => {
                    let code = event
                        .get("code")
                        .and_then(Value::as_str)
                        .filter(|code| {
                            code.len() <= 64
                                && code.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                        })
                        .unwrap_or("AI_WORKER_FAILED");
                    let message = event
                        .get("message")
                        .and_then(Value::as_str)
                        .filter(|message| message.len() <= 200 && !message.contains('/'))
                        .unwrap_or("AI 处理失败");
                    return Err(AppError::new(code, message));
                }
                _ => {
                    return Err(AppError::new(
                        "AI_WORKER_PROTOCOL_INVALID",
                        "AI 工作程序输出格式无效",
                    ))
                }
            }
        }
        // A result alone is insufficient: the worker must also exit cleanly.
        loop {
            if cancellation.is_cancelled() {
                return Err(AppError::new("AI_CANCELLED", "AI 媒体任务已取消"));
            }
            if let Some(status) = child.try_wait().map_err(|error| {
                AppError::with_cause("AI_WORKER_FAILED", "AI 工作程序状态异常", error.to_string())
            })? {
                if !status.success() {
                    return Err(AppError::new(
                        "AI_WORKER_FAILED",
                        format!("AI 工作程序异常退出（{status}）"),
                    ));
                }
                return result
                    .ok_or_else(|| AppError::new("AI_WORKER_FAILED", "AI 工作程序未返回结果"));
            }
            thread::sleep(Duration::from_millis(50));
        }
    })();
    if outcome.is_err() {
        cancellation.kill();
        let _ = child.kill();
        let _ = child.wait();
    }
    cancellation.clear_child(child_id);
    drop(receiver);
    let _ = stdout_reader.join();
    let _ = stderr_reader.join();
    outcome.map_err(|error| {
        let tail = diagnostics.lock().unwrap();
        let diagnostic = decode_worker_stdout_line(&tail)
            .unwrap_or_else(|| String::from_utf8_lossy(&tail).into_owned());
        AppError::with_cause(
            &error.code,
            &error.message,
            format!("{error:?}\n{diagnostic}"),
        )
    })
}

fn ascii_worker_request(request: &Value) -> String {
    use std::fmt::Write;
    let mut ascii = String::new();
    for unit in request.to_string().encode_utf16() {
        if unit < 128 {
            ascii.push(unit as u8 as char);
        } else {
            write!(&mut ascii, "\\u{unit:04x}").expect("write to string");
        }
    }
    ascii.push('\n');
    ascii
}

fn decode_worker_stdout_line(buffer: &[u8]) -> Option<String> {
    let mut line = buffer;
    if let Some(without_lf) = line.strip_suffix(b"\n") {
        line = without_lf;
    }
    if let Some(without_cr) = line.strip_suffix(b"\r") {
        line = without_cr;
    }
    if line.is_empty() {
        return None;
    }
    if let Ok(text) = std::str::from_utf8(line) {
        return Some(text.trim_start_matches('\u{feff}').to_owned());
    }
    let (text, invalid) = encoding_rs::GBK.decode_without_bom_handling(line);
    (!invalid).then(|| text.into_owned())
}

fn worker_output_path(value: &Value, field: &str, root: &Path) -> Result<PathBuf, AppError> {
    let raw = value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::new("AI_OUTPUT_INVALID", "AI 工作程序输出缺少文件"))?;
    let path = fs::canonicalize(raw).map_err(ai_io)?;
    let root = fs::canonicalize(root).map_err(ai_io)?;
    if !path.starts_with(root) || !regular_nonempty(&path) {
        return Err(AppError::new(
            "AI_OUTPUT_INVALID",
            "AI 工作程序输出越界或无效",
        ));
    }
    Ok(path)
}

fn regular_nonempty(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() > 0
    })
}

fn ai_io(error: impl std::fmt::Display) -> AppError {
    AppError::with_cause("AI_IO", "AI 媒体处理文件操作失败", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        decode_worker_stdout_line, runtime_candidates_for, select_subtitle_source, SubtitleSource,
    };

    #[test]
    fn concurrent_jobs_can_initialize_a_fresh_work_directory() {
        let fixture = super::JobTemp::create(&std::env::temp_dir()).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(12));
        let workers = (0..12)
            .map(|_| {
                let root = fixture.output.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    super::JobTemp::create(&root)
                })
            })
            .collect::<Vec<_>>();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert!(
            results.iter().all(Result::is_ok),
            "{:?}",
            results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn merged_request_does_not_reuse_unidentified_episode_one_outputs() {
        use super::*;
        let temp = JobTemp::create(&std::env::temp_dir()).unwrap();
        let destination = safe_output_directory(&temp.root, "音频分离").unwrap();
        for name in [
            "001_htdemucs_人声.wav",
            "001_htdemucs_背景音乐.wav",
            "001_htdemucs_去背景音乐.mp4",
        ] {
            fs::write(destination.join(name), b"old episode one").unwrap();
        }
        let request = ValidatedAIJobRequest {
            book_id: "test".into(),
            title: "test".into(),
            kind: MediaJobKind::SeparateBackgroundMusic,
            scope: super::super::MediaJobScope::Merged,
            series_root: temp.root.clone(),
            inputs: vec![],
            model: "htdemucs".into(),
            device: "cpu".into(),
            dedupe_key: "new-source".into(),
        };
        let tools = MediaTools::from_test_paths(
            PathBuf::from("missing-ffmpeg"),
            PathBuf::from("missing-ffprobe"),
        );
        let cancellation = CancellationToken::default();
        let context = AIItemContext {
            tools: &tools,
            runtime: Path::new("missing-worker"),
            model_root: &temp.root,
            request: &request,
            cancellation: &cancellation,
            cpu_threads: None,
        };
        assert!(process_separation(
            &context,
            1,
            &temp.root.join("new-merged.mp4"),
            &mut |_, _| {}
        )
        .is_err());
    }

    #[test]
    fn windows_runtime_candidates_preserve_modern_legacy_and_cpu_fallbacks() {
        assert_eq!(
            runtime_candidates_for("windows", "auto"),
            ["runtime-modern", "runtime-legacy", "runtime-cpu"]
        );
        assert_eq!(
            runtime_candidates_for("windows", "cuda"),
            ["runtime-modern", "runtime-legacy", "runtime"]
        );
        assert_eq!(
            runtime_candidates_for("windows", "cpu"),
            ["runtime-cpu", "runtime-modern", "runtime-legacy"]
        );
        assert_eq!(runtime_candidates_for("macos", "cuda"), ["runtime"]);
    }

    #[cfg(unix)]
    fn worker_fixture(script: &str) -> (super::JobTemp, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let temp = super::JobTemp::create(&std::env::temp_dir()).unwrap();
        let runtime = temp.root.join("worker");
        std::fs::write(&runtime, format!("#!/bin/sh\ncat >/dev/null\n{script}\n")).unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        (temp, runtime)
    }

    #[cfg(unix)]
    #[test]
    fn separation_worker_receives_its_thread_budget() {
        let (temp, runtime) = worker_fixture(
            r#"
[ "$OMP_NUM_THREADS" = 2 ] || exit 21
[ "$MKL_NUM_THREADS" = 2 ] || exit 22
[ "$OPENBLAS_NUM_THREADS" = 2 ] || exit 23
echo '{"type":"result","outputs":{"ok":true}}'
"#,
        );
        let result = super::run_worker(
            super::WorkerInvocation {
                runtime: &runtime,
                args: &[],
                ffmpeg: None,
                job_id: "budget-test",
                operation: "separate",
                input: &runtime,
                output: &temp.output,
                options: serde_json::json!({"device":"cpu", "cpuThreads":2}),
            },
            &super::CancellationToken::default(),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(result, serde_json::json!({"ok": true}));
    }

    #[cfg(unix)]
    #[test]
    fn transcription_uses_cpu_and_the_bundled_ffmpeg_with_existing_runtime() {
        use std::os::unix::fs::PermissionsExt;
        let (temp, runtime) = worker_fixture("");
        let ffmpeg = temp.root.join("ffmpeg");
        std::fs::write(&ffmpeg, "#!/bin/sh\necho bundled-ffmpeg\n").unwrap();
        std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(
            &runtime,
            r#"#!/bin/sh
request=$(/bin/cat)
case "$request" in
  *'"device":"cpu"'*) ;;
  *) exit 23 ;;
esac
[ "$(ffmpeg)" = bundled-ffmpeg ] || exit 24
echo "Detected language: Chinese"
echo '{"type":"result","outputs":{"ok":true}}'
"#,
        )
        .unwrap();
        let result = super::run_worker(
            super::WorkerInvocation {
                runtime: &runtime,
                args: &[],
                ffmpeg: Some(&ffmpeg),
                job_id: "test",
                operation: "transcribe",
                input: &runtime,
                output: &temp.output,
                options: serde_json::json!({"device":"auto", "model":"small"}),
            },
            &super::CancellationToken::default(),
            &mut |_, _| {},
        );
        assert_eq!(result.unwrap(), serde_json::json!({"ok": true}));
    }

    #[test]
    #[ignore = "requires explicit installed runtime/model and a local speech WAV"]
    fn installed_runtime_transcribes_local_speech_through_native_command() {
        let path = |key| std::path::PathBuf::from(std::env::var_os(key).expect(key));
        let runtime = path("HONGGUO_TEST_AI_RUNTIME");
        let ffmpeg = path("HONGGUO_TEST_FFMPEG");
        let model_root = path("HONGGUO_TEST_WHISPER_MODEL_ROOT");
        let temp = super::JobTemp::create(&std::env::temp_dir()).unwrap();
        let input = temp.root.join("input.wav");
        std::fs::copy(path("HONGGUO_TEST_SPEECH_WAV"), &input).unwrap();
        let outputs = super::run_worker(
            super::WorkerInvocation {
                runtime: &runtime, args: &[], ffmpeg: Some(&ffmpeg), job_id: "test", operation: "transcribe",
                input: &input, output: &temp.output,
                options: serde_json::json!({"model":"small", "device":"auto", "modelRoot":model_root}),
            },
            &super::CancellationToken::default(),
            &mut |_, _| {},
        ).unwrap();
        let srt = super::worker_output_path(&outputs, "srtPath", &temp.output).unwrap();
        super::validate_srt(&srt).unwrap();
        assert!(outputs["segmentCount"].as_u64().unwrap() > 0);
    }

    #[cfg(unix)]
    #[test]
    fn worker_streams_progress_before_exit_and_drains_more_than_pipe_capacity() {
        let (temp, runtime) = worker_fixture(
            "echo '{\"type\":\"progress\",\"stage\":\"first\",\"percent\":10}'\n\
             while [ ! -f \"$0.ready\" ]; do sleep 0.01; done\n\
             i=0\n\
             while [ $i -lt 3000 ]; do\n\
               echo '{\"type\":\"progress\",\"stage\":\"chunk\",\"percent\":50}'\n\
               i=$((i+1))\n\
             done\n\
             echo '{\"type\":\"result\",\"outputs\":{\"ok\":true}}'",
        );
        let cancellation = super::CancellationToken::default();
        let timeout_token = cancellation.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let watchdog = std::thread::spawn(move || {
            if done_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .is_err()
            {
                timeout_token.cancel();
            }
        });
        let mut events = 0;
        let result = super::run_worker(
            super::WorkerInvocation {
                runtime: &runtime,
                args: &[],
                ffmpeg: None,
                job_id: "test",
                operation: "separate",
                input: &runtime,
                output: &temp.output,
                options: serde_json::json!({}),
            },
            &cancellation,
            &mut |stage, _| {
                if stage == "first" {
                    std::fs::write(runtime.with_extension("ready"), b"ready").unwrap();
                }
                events += 1;
            },
        );
        let _ = done_tx.send(());
        watchdog.join().unwrap();
        assert_eq!(result.unwrap(), serde_json::json!({"ok": true}));
        assert_eq!(events, 3001);
    }

    #[cfg(unix)]
    #[test]
    fn worker_nonzero_exit_cannot_publish_a_result() {
        let (temp, runtime) =
            worker_fixture("echo '{\"type\":\"result\",\"outputs\":{\"ok\":true}}'\nexit 2");
        let result = super::run_worker(
            super::WorkerInvocation {
                runtime: &runtime,
                args: &[],
                ffmpeg: None,
                job_id: "test",
                operation: "separate",
                input: &runtime,
                output: &temp.output,
                options: serde_json::json!({}),
            },
            &super::CancellationToken::default(),
            &mut |_, _| {},
        );
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn worker_can_be_cancelled_after_live_progress() {
        let (temp, runtime) =
            worker_fixture("echo '{\"type\":\"progress\",\"percent\":10}'\nexec sleep 10");
        let cancellation = super::CancellationToken::default();
        let started = std::time::Instant::now();
        let result = super::run_worker(
            super::WorkerInvocation {
                runtime: &runtime,
                args: &[],
                ffmpeg: None,
                job_id: "test",
                operation: "separate",
                input: &runtime,
                output: &temp.output,
                options: serde_json::json!({}),
            },
            &cancellation,
            &mut |_, _| cancellation.cancel(),
        );
        assert_eq!(result.unwrap_err().code, "AI_CANCELLED");
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn audio_extraction_preserves_concat_timestamp_gaps_before_separation() {
        use std::os::unix::fs::PermissionsExt;

        let temp = super::JobTemp::create(&std::env::temp_dir()).unwrap();
        let ffmpeg = temp.root.join("ffmpeg");
        let arguments = temp.root.join("arguments");
        std::fs::write(
            &ffmpeg,
            format!(
                "#!/bin/sh\nlast=''\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{}'; last=$arg; done\nprintf x > \"$last\"\n",
                arguments.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o700)).unwrap();
        let tools = super::MediaTools::from_test_paths(ffmpeg.clone(), ffmpeg);
        let input = temp.root.join("input.mp4");
        let output = temp.root.join("output.wav");
        std::fs::write(&input, b"input").unwrap();

        super::extract_audio(&tools, &input, &output, &super::CancellationToken::new()).unwrap();

        let arguments = std::fs::read_to_string(arguments).unwrap();
        assert!(arguments.contains("-af\naresample=async=1:first_pts=0\n"));
        assert!(output.is_file());
    }

    #[test]
    fn embedded_text_wins_and_bitmap_or_missing_tracks_use_whisper() {
        assert_eq!(
            select_subtitle_source(&["subrip".into()], false),
            SubtitleSource::EmbeddedText
        );
        assert_eq!(
            select_subtitle_source(&["hdmv_pgs_subtitle".into()], false),
            SubtitleSource::WhisperOriginalAudio
        );
        assert_eq!(
            select_subtitle_source(&[], false),
            SubtitleSource::WhisperOriginalAudio
        );
    }

    #[test]
    fn compatible_vocals_are_preferred_for_whisper() {
        assert_eq!(
            select_subtitle_source(&[], true),
            SubtitleSource::WhisperVocals
        );
        assert_eq!(
            select_subtitle_source(&["ass".into()], true),
            SubtitleSource::EmbeddedText
        );
    }

    #[test]
    fn worker_stdout_skips_non_utf8_and_keeps_utf8_json() {
        assert_eq!(decode_worker_stdout_line(b"\xff\xfe\n"), None);
        assert_eq!(decode_worker_stdout_line(b"\n"), None);
        assert_eq!(
            decode_worker_stdout_line(b"{\"type\":\"result\"}\r\n").as_deref(),
            Some("{\"type\":\"result\"}")
        );
        let chinese = "{\"path\":\"D:\\\\红果下载\"}\n";
        assert_eq!(
            decode_worker_stdout_line(chinese.as_bytes()).as_deref(),
            Some("{\"path\":\"D:\\\\红果下载\"}")
        );
    }

    fn python_program() -> &'static str {
        if cfg!(windows) {
            "python"
        } else {
            "python3"
        }
    }

    fn python_worker_fixture(body: &str) -> (super::JobTemp, std::path::PathBuf) {
        let temp = super::JobTemp::create(&std::env::temp_dir()).unwrap();
        let script = temp.root.join("worker.py");
        std::fs::write(
            &script,
            format!(
                "# -*- coding: utf-8 -*-\nimport os\nimport sys\n_request_bytes = sys.stdin.buffer.read()\n{body}\n"
            ),
        )
        .unwrap();
        (temp, script)
    }

    fn run_python_worker(
        body: &str,
        operation: &str,
    ) -> Result<serde_json::Value, crate::AppError> {
        let (temp, script) = python_worker_fixture(body);
        let script = script.to_str().unwrap().to_string();
        super::run_worker(
            super::WorkerInvocation {
                runtime: std::path::Path::new(python_program()),
                args: &[script.as_str()],
                ffmpeg: None,
                job_id: "test",
                operation,
                input: &temp.root,
                output: &temp.output,
                options: serde_json::json!({"label": "红果 中文 😀"}),
            },
            &super::CancellationToken::default(),
            &mut |_, _| {},
        )
    }

    #[test]
    fn worker_request_preserves_unicode_with_legacy_gbk_stdin() {
        let result = run_python_worker(
            r#"import json
request = json.loads(_request_bytes.decode('gbk'))
assert request['options']['label'] == '红果 中文 😀'
print(json.dumps({'type': 'result', 'outputs': {'ok': True}}))"#,
            "separate",
        );
        assert_eq!(result.unwrap(), serde_json::json!({"ok": true}));
    }

    #[test]
    fn worker_round_trips_real_unicode_paths_with_legacy_stdio() {
        let (temp, script) = python_worker_fixture(
            r#"import json
from pathlib import Path
request = json.loads(_request_bytes.decode('gbk'))
assert Path(request['inputPath']).read_text(encoding='utf8') == '中文路径'
output = Path(request['outputDir']) / '人声.wav'
output.write_bytes(b'wave fixture')
event = {'type': 'result', 'outputs': {'vocalsPath': str(output.resolve())}}
sys.stdout.buffer.write((json.dumps(event, ensure_ascii=False) + '\n').encode('gbk'))"#,
        );
        let root = temp.root.join("中文 空格目录");
        std::fs::create_dir(&root).unwrap();
        let output = root.join("分离 输出");
        std::fs::create_dir(&output).unwrap();
        let input = root.join("合并 视频.wav");
        std::fs::write(&input, "中文路径").unwrap();
        let input = std::fs::canonicalize(input).unwrap();
        let output = std::fs::canonicalize(output).unwrap();
        let results = super::run_worker(
            super::WorkerInvocation {
                runtime: std::path::Path::new(python_program()),
                args: &[script.to_str().unwrap()],
                ffmpeg: None,
                job_id: "unicode-paths",
                operation: "separate",
                input: &input,
                output: &output,
                options: serde_json::json!({}),
            },
            &super::CancellationToken::default(),
            &mut |_, _| {},
        )
        .unwrap();
        let actual = super::worker_output_path(&results, "vocalsPath", &output).unwrap();
        assert_eq!(std::fs::read(actual).unwrap(), b"wave fixture");
    }

    #[test]
    #[ignore = "requires an installed runtime, Demucs model and bundled FFmpeg"]
    fn installed_runtime_separates_video_in_unicode_directory() {
        use super::*;
        let env_path = |name| std::path::PathBuf::from(std::env::var_os(name).expect(name));
        let runtime = env_path("HONGGUO_TEST_AI_RUNTIME");
        let model_root = env_path("HONGGUO_TEST_DEMUCS_MODEL");
        let tools = MediaTools::from_test_paths(
            env_path("HONGGUO_TEST_FFMPEG"),
            env_path("HONGGUO_TEST_FFPROBE"),
        );
        let temp = JobTemp::create(&std::env::temp_dir()).unwrap();
        let root = temp.root.join("中文 空格剧目");
        std::fs::create_dir(&root).unwrap();
        let input = root.join("合并 视频.mp4");
        assert!(tools
            .ffmpeg_command()
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=s=160x120:r=25",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=220:sample_rate=44100",
                "-t",
                "1",
                "-c:v",
                "libx264",
                "-c:a",
                "aac",
            ])
            .arg(&input)
            .status()
            .unwrap()
            .success());
        let request = super::super::validate_ai_request(
            &super::super::StartAIJobRequest {
                book_id: "test".into(),
                title: "中文 空格剧目".into(),
                series_root: root,
                scope: super::super::MediaJobScope::Merged,
                inputs: vec![super::super::StartMergeInput {
                    episode_index: 1,
                    path: input,
                }],
                model: "htdemucs".into(),
                device: "cpu".into(),
            },
            MediaJobKind::SeparateBackgroundMusic,
        )
        .unwrap();
        let cancellation = CancellationToken::default();
        let context = AIItemContext {
            tools: &tools,
            runtime: &runtime,
            model_root: &model_root,
            request: &request,
            cancellation: &cancellation,
            cpu_threads: None,
        };
        // Simulate an older job interrupted after only its first file was saved.
        let legacy = safe_output_directory(&request.series_root, "音频分离").unwrap();
        let partial = legacy.join("001_htdemucs_人声.wav");
        std::fs::write(&partial, b"old incomplete job").unwrap();
        let outputs =
            process_separation(&context, 1, &request.inputs[0].path, &mut |_, _| {}).unwrap();
        assert_eq!(outputs.len(), 3);
        assert_eq!(std::fs::read(&partial).unwrap(), b"old incomplete job");
        let cached =
            process_separation(&context, 1, &request.inputs[0].path, &mut |_, _| {}).unwrap();
        assert_eq!(cached, outputs);
        for output in &outputs {
            assert!(regular_nonempty(&output.path));
            assert!(output.path.starts_with(&request.series_root));
        }
        let mut episode_request = request.clone();
        episode_request.scope = super::super::MediaJobScope::Episodes;
        let episode_context = AIItemContext {
            request: &episode_request,
            ..context
        };
        let episode_outputs = process_separation(
            &episode_context,
            1,
            &episode_request.inputs[0].path,
            &mut |_, _| {},
        )
        .unwrap();
        assert_ne!(outputs[2].path, episode_outputs[2].path);
    }

    #[test]
    fn worker_accepts_legacy_gbk_json_paths_and_errors() {
        let outputs = run_python_worker(
            r#"import json
event = {'type': 'result', 'outputs': {'path': 'D:\\红果下载\\中文 空格\\人声.wav'}}
sys.stdout.buffer.write((json.dumps(event, ensure_ascii=False) + '\n').encode('gbk'))"#,
            "separate",
        )
        .unwrap();
        assert_eq!(outputs["path"], "D:\\红果下载\\中文 空格\\人声.wav");
        let error = run_python_worker(
            r#"import json
event = {'type': 'error', 'code': 'AI_MODEL_INVALID', 'message': '模型文件缺失'}
sys.stdout.buffer.write((json.dumps(event, ensure_ascii=False) + '\n').encode('gbk'))
sys.exit(2)"#,
            "separate",
        )
        .unwrap_err();
        assert_eq!(error.code, "AI_MODEL_INVALID");
        assert_eq!(error.message, "模型文件缺失");
    }

    #[test]
    fn worker_reports_exit_status_and_bounded_stderr_diagnostics() {
        let error = run_python_worker(
            "sys.stderr.buffer.write(b'x' * 200000 + b'\\nDLL load failed: missing dependency\\n')\nsys.exit(23)",
            "separate",
        ).unwrap_err();
        let cause = format!("{error:?}");
        assert!(cause.contains("23"));
        assert!(cause.contains("DLL load failed"));
        assert!(cause.len() < 17000);
    }

    #[test]
    fn worker_accepts_utf8_chinese_json_after_non_utf8_noise() {
        let result = run_python_worker(
            "sys.stdout.buffer.write(bytes([0xFF, 0xFE, 10]))\n\
             sys.stdout.buffer.write(('{\\\"type\\\":\\\"result\\\",\\\"outputs\\\":{\\\"ok\\\":true,\\\"path\\\":\\\"D:\\\\\\\\红果下载\\\\\\\\a\\\"}}\\n').encode('utf-8'))\n\
             sys.stdout.buffer.flush()\n",
            "separate",
        );
        assert_eq!(
            result.unwrap(),
            serde_json::json!({"ok": true, "path": "D:\\红果下载\\a"})
        );
    }

    #[test]
    fn worker_forces_utf8_python_stdio_environment() {
        let result = run_python_worker(
            "encoding = (os.environ.get('PYTHONIOENCODING') or '').lower().replace('-', '')\n\
             assert os.environ.get('PYTHONUTF8') == '1' and encoding.startswith('utf8')\n\
             sys.stdout.buffer.write(b'{\\\"type\\\":\\\"result\\\",\\\"outputs\\\":{\\\"ok\\\":true}}\\n')\n\
             sys.stdout.buffer.flush()\n",
            "separate",
        );
        assert_eq!(result.unwrap(), serde_json::json!({"ok": true}));
    }

    #[test]
    fn windows_nsis_hooks_stop_running_app_before_overwrite() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let hooks = std::fs::read_to_string(manifest_dir.join("windows/installer-hooks.nsh"))
            .expect("installer hooks");
        let config: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(manifest_dir.join("tauri.conf.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            config["bundle"]["windows"]["nsis"]["installerHooks"],
            "windows/installer-hooks.nsh"
        );
        let release: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(manifest_dir.join("tauri.release.conf.json")).unwrap(),
        )
        .unwrap();
        assert!(release.pointer("/bundle/windows").is_none());
        assert!(hooks.contains("NSIS_HOOK_PREINSTALL"));
        assert!(hooks.contains("NSIS_HOOK_PREUNINSTALL"));
        assert!(hooks.contains("${MAINBINARYNAME}.exe"));
        assert!(hooks.contains("红果下载.exe"));
        assert!(hooks.contains("hongguo-api.exe"));
        assert!(hooks.contains("hongguo-ai-worker.exe"));
        let taskkill = hooks
            .lines()
            .filter(|line| !line.trim_start().starts_with(';'))
            .collect::<Vec<_>>()
            .join("\n")
            .to_ascii_lowercase();
        assert!(taskkill.contains("taskkill"));
        assert!(!taskkill.contains("ffmpeg.exe"));
        assert!(!taskkill.contains("ffprobe.exe"));
    }
}
