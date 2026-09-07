use super::{
    AIExecutionResult, AIExecutor, CancellationToken, ComponentManager, MediaJobKind,
    MediaJobOutput, MediaJobOutputKind, MediaTools, MergeProgress, ValidatedAIJobRequest,
};
use crate::AppError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Duration,
};

static AI_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
    fn execute(
        &self,
        request: ValidatedAIJobRequest,
        cancellation: &CancellationToken,
        progress: &mut dyn FnMut(MergeProgress),
    ) -> Result<AIExecutionResult, AppError> {
        let tools = self.tools.as_ref().map_err(Clone::clone)?;
        let model_id = match request.kind {
            MediaJobKind::SeparateBackgroundMusic => format!("demucs-{}", request.model),
            MediaJobKind::ExtractSubtitles => format!("whisper-{}", request.model),
            MediaJobKind::Merge => {
                return Err(AppError::new("AI_REQUEST_INVALID", "AI 任务类型无效"))
            }
        };
        let runtime = self.components.resolve_installed("runtime")?;
        let model = self.components.resolve_installed(&model_id)?;
        let _in_use =
            ComponentUseGuard::new(self.components.clone(), vec!["runtime".into(), model_id]);
        let mut outputs = Vec::new();
        let total = request.inputs.len().max(1) as f64;
        let context = AIItemContext {
            tools,
            runtime: &runtime.entrypoint,
            model_root: &model.root,
            request: &request,
            cancellation,
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
}

fn process_separation(
    context: &AIItemContext<'_>,
    episode_index: u32,
    input: &Path,
    progress: &mut dyn FnMut(String, f64),
) -> Result<Vec<MediaJobOutput>, AppError> {
    let request = context.request;
    let destination = safe_output_directory(&request.series_root, "音频分离")?;
    let prefix = format!("{episode_index:03}_{}", request.model);
    let vocals_destination = destination.join(format!("{prefix}_人声.wav"));
    let music_destination = destination.join(format!("{prefix}_背景音乐.wav"));
    let video_destination = destination.join(format!("{prefix}_去背景音乐.mp4"));
    if [&vocals_destination, &music_destination, &video_destination]
        .iter()
        .all(|path| regular_nonempty(path))
    {
        return Ok(separation_outputs(
            episode_index,
            vocals_destination,
            music_destination,
            video_destination,
        ));
    }
    let temp = JobTemp::create(&request.series_root)?;
    let wav = temp.root.join("input.wav");
    extract_audio(context.tools, input, &wav, context.cancellation)?;
    progress("separating".into(), 10.0);
    let worker_outputs = run_worker(
        WorkerInvocation {
            runtime: context.runtime,
            ffmpeg: Some(context.tools.ffmpeg()),
            job_id: &request.dedupe_key,
            operation: "separate",
            input: &wav,
            output: &temp.output,
            options: json!({"model": request.model, "device": "auto", "modelRoot": context.model_root}),
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
    publish_file(&vocals, &vocals_destination)?;
    publish_file(&music, &music_destination)?;
    publish_file(&remuxed, &video_destination)?;
    Ok(separation_outputs(
        episode_index,
        vocals_destination,
        music_destination,
        video_destination,
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
    let destination = safe_output_directory(&request.series_root, "字幕")?;
    let final_path = destination.join(format!("{episode_index:03}_{}.srt", request.model));
    if regular_nonempty(&final_path) {
        validate_srt(&final_path)?;
        return Ok(vec![MediaJobOutput {
            episode_index,
            kind: MediaJobOutputKind::Subtitles,
            path: final_path,
            source: None,
        }]);
    }
    let temp = JobTemp::create(&request.series_root)?;
    let tracks = subtitle_tracks(context.tools, input)?;
    let vocals = find_vocals(&request.series_root, episode_index);
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
                    ffmpeg: Some(context.tools.ffmpeg()),
                    job_id: &request.dedupe_key,
                    operation: "transcribe",
                    input: &wav,
                    output: &temp.output,
                    options: json!({"model": request.model, "device": "auto", "modelRoot": context.model_root}),
                },
                context.cancellation,
                progress,
            )?;
            let worker_srt = worker_output_path(&outputs, "srtPath", &temp.output)?;
            fs::copy(worker_srt, &temporary_srt).map_err(ai_io)?;
        }
    }
    validate_srt(&temporary_srt)?;
    publish_file(&temporary_srt, &final_path)?;
    Ok(vec![MediaJobOutput {
        episode_index,
        kind: MediaJobOutputKind::Subtitles,
        path: final_path,
        source: Some(
            match source {
                SubtitleSource::EmbeddedText => "embeddedText",
                SubtitleSource::WhisperOriginalAudio => "whisperOriginalAudio",
                SubtitleSource::WhisperVocals => "whisperVocals",
            }
            .into(),
        ),
    }])
}

fn safe_output_directory(root: &Path, name: &str) -> Result<PathBuf, AppError> {
    let canonical_root = fs::canonicalize(root).map_err(ai_io)?;
    let path = canonical_root.join(name);
    if path.exists() {
        let metadata = fs::symlink_metadata(&path).map_err(ai_io)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(AppError::new("AI_OUTPUT_INVALID", "AI 输出目录不安全"));
        }
    } else {
        fs::create_dir(&path).map_err(ai_io)?;
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
        let parent = series_root.join(".hongguo-ai-work");
        fs::create_dir_all(&parent).map_err(ai_io)?;
        let sequence = AI_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = parent.join(format!("{}-{sequence}", std::process::id()));
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
    let mut command = Command::new(invocation.runtime);
    if invocation.operation == "transcribe" {
        // Whisper launches `ffmpeg` by name. Finder-launched applications do not
        // inherit a shell PATH; use only the verified bundle and system tools.
        let ffmpeg_dir = invocation
            .ffmpeg
            .and_then(Path::parent)
            .ok_or_else(|| AppError::new("MEDIA_TOOL_MISSING", "语音转写缺少打包的 FFmpeg"))?;
        let mut search_paths = vec![ffmpeg_dir.to_path_buf()];
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
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cancellation.prepare_command(&mut command);
    let mut child = command.spawn().map_err(|error| {
        AppError::with_cause(
            "AI_WORKER_FAILED",
            "无法启动 AI 工作程序",
            error.to_string(),
        )
    })?;
    cancellation.register_child(&mut child)?;
    let child_id = child.id();
    if let Some(mut stdin) = child.stdin.take() {
        serde_json::to_writer(&mut stdin, &request)
            .and_then(|_| stdin.write_all(b"\n").map_err(serde_json::Error::io))
            .map_err(|error| {
                AppError::with_cause("AI_WORKER_FAILED", "无法发送 AI 请求", error.to_string())
            })?;
    }
    // Drain stdout while the process runs. Waiting for exit first both hides
    // progress and deadlocks once the worker fills the OS pipe buffer.
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::new("AI_WORKER_FAILED", "AI 工作程序输出不可用"))?;
    let (sender, receiver) = mpsc::sync_channel(32);
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let outcome = (|| {
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
                    return Err(AppError::new("AI_WORKER_FAILED", "AI 工作程序异常退出"));
                }
                return result
                    .ok_or_else(|| AppError::new("AI_WORKER_FAILED", "AI 工作程序未返回结果"));
            }
            thread::sleep(Duration::from_millis(50));
        }
    })();
    if outcome.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    cancellation.clear_child(child_id);
    outcome
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

fn publish_file(source: &Path, destination: &Path) -> Result<(), AppError> {
    if destination.exists() {
        return Err(AppError::new(
            "AI_OUTPUT_EXISTS",
            "AI 输出文件已存在，未覆盖原文件",
        ));
    }
    fs::rename(source, destination)
        .or_else(|_| fs::copy(source, destination).map(|_| ()))
        .map_err(ai_io)
}

fn regular_nonempty(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() > 0
    })
}

fn find_vocals(root: &Path, episode_index: u32) -> Option<PathBuf> {
    let prefix = format!("{episode_index:03}_");
    fs::read_dir(root.join("音频分离"))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with("_人声.wav"))
                && regular_nonempty(path)
        })
}

fn ai_io(error: impl std::fmt::Display) -> AppError {
    AppError::with_cause("AI_IO", "AI 媒体处理文件操作失败", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{select_subtitle_source, SubtitleSource};

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
                runtime: &runtime, ffmpeg: Some(&ffmpeg), job_id: "test", operation: "transcribe",
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
}
