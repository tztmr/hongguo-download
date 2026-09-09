use super::{
    hardware::{probe_video_hardware, select_video_encoder, VideoEncoder},
    model::{MergeConflictPolicy, MergeInput, MergeMode, MergeQuality, MergeRequest},
    process_control::CancellationToken,
    tools::MediaTools,
};
use crate::AppError;
use serde::Deserialize;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::windows::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    process::{Child, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSignature {
    pub codec_name: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<String>,
    pub time_base: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaProbe {
    pub video: StreamSignature,
    pub audio: Option<StreamSignature>,
    pub duration_seconds: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergeProgress {
    pub stage: String,
    pub percent: f64,
    pub terminal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeResult {
    pub output_path: PathBuf,
}

#[derive(Deserialize)]
struct RawProbe {
    streams: Vec<RawStream>,
    format: RawFormat,
}

#[derive(Deserialize)]
struct RawStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
    time_base: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u16>,
}

#[derive(Deserialize)]
struct RawFormat {
    duration: Option<String>,
}

pub fn can_stream_copy(probes: &[MediaProbe]) -> bool {
    let Some(first) = probes.first() else {
        return false;
    };
    probes
        .iter()
        .skip(1)
        .all(|probe| probe.video == first.video && probe.audio == first.audio)
}

pub fn probe_media(tools: &MediaTools, path: &Path) -> Result<MediaProbe, AppError> {
    let output = tools
        .ffprobe_command()
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            AppError::with_cause(
                "FFPROBE_FAILED",
                "无法运行打包的媒体检测工具",
                error.to_string(),
            )
        })?;
    if !output.status.success() {
        return Err(AppError::new("FFPROBE_FAILED", "媒体文件无法读取"));
    }
    parse_probe_json(&output.stdout)
}

pub fn run_merge<F>(
    tools: &MediaTools,
    request: MergeRequest,
    cancellation: &CancellationToken,
    mut progress: F,
) -> Result<MergeResult, AppError>
where
    F: FnMut(MergeProgress),
{
    progress(event("probing", 0.0, false));
    let result = run_merge_inner(tools, request, cancellation, &mut progress);
    match &result {
        Ok(_) => progress(event("completed", 100.0, true)),
        Err(error) => progress(event(
            if error.code == "MERGE_CANCELLED" {
                "cancelled"
            } else {
                "failed"
            },
            0.0,
            true,
        )),
    }
    result
}

fn run_merge_inner(
    tools: &MediaTools,
    request: MergeRequest,
    cancellation: &CancellationToken,
    progress: &mut dyn FnMut(MergeProgress),
) -> Result<MergeResult, AppError> {
    if request.inputs.is_empty() {
        return Err(AppError::new("MERGE_INPUT_INVALID", "合并输入不得为空"));
    }
    validate_output_name(&request.output_file_name)?;
    let root = fs::canonicalize(&request.series_root).map_err(output_invalid)?;
    ensure_real_directory(&root)?;
    let mut inputs = request.inputs.clone();
    inputs.sort_by_key(|input| input.episode_index);
    validate_inputs(&root, &inputs)?;

    let output_directory = root.join("合并视频");
    if output_directory.exists() {
        ensure_real_directory(&output_directory)?;
    } else {
        fs::create_dir(&output_directory).map_err(output_invalid)?;
    }
    let output_path = output_directory.join(&request.output_file_name);
    if output_path.exists() && request.conflict_policy == MergeConflictPolicy::FailIfExists {
        return Err(AppError::new(
            "MERGE_OUTPUT_EXISTS",
            "合并视频已存在，不能重复合并",
        ));
    }

    let temp = TempDirectory::create(&root)?;
    let concat = temp.path.join("concat.txt");
    write_concat_file(&concat, &inputs)?;
    let mut probes = Vec::with_capacity(inputs.len());
    for input in &inputs {
        check_cancelled(cancellation)?;
        probes.push(probe_media(tools, &input.path)?);
    }
    validate_inputs(&root, &inputs)?;
    let compatible = can_stream_copy(&probes);
    let needs_transcode = match request.mode {
        Some(MergeMode::Copy) => {
            if !compatible {
                return Err(AppError::new(
                    "MERGE_TRANSCODE_REQUIRED",
                    "输入媒体参数不一致，需要开启 H.264 转码",
                ));
            }
            false
        }
        Some(MergeMode::Transcode) => true,
        Some(MergeMode::Auto) => !compatible,
        None => request.transcode_h264,
    };
    if !needs_transcode && !compatible {
        return Err(AppError::new(
            "MERGE_TRANSCODE_REQUIRED",
            "输入媒体参数不一致，需要开启 H.264 转码",
        ));
    }

    let temp_output = temp.path.join("merged.mp4");
    let expected_duration = probes
        .iter()
        .map(|probe| probe.duration_seconds)
        .sum::<f64>();
    let encoder = if needs_transcode {
        let hardware = probe_video_hardware(tools);
        select_video_encoder(true, hardware.nvenc_available)
    } else {
        VideoEncoder::Copy
    };
    progress(event(encoder_stage(encoder), 0.0, false));
    let outcome = run_ffmpeg(
        tools,
        &concat,
        &temp_output,
        encoder,
        request.quality,
        expected_duration,
        cancellation,
        progress,
    )?;
    if !outcome.success && encoder == VideoEncoder::Nvenc && nvenc_runtime_failure(&outcome.stderr)
    {
        let _ = fs::remove_file(&temp_output);
        progress(event("NVENC 不可用，改用 CPU 重新编码", 0.0, false));
        let fallback = run_ffmpeg(
            tools,
            &concat,
            &temp_output,
            VideoEncoder::Libx264,
            request.quality,
            expected_duration,
            cancellation,
            progress,
        )?;
        if !fallback.success {
            return Err(ffmpeg_error(fallback.stderr));
        }
    } else if !outcome.success {
        return Err(ffmpeg_error(outcome.stderr));
    }

    let merged = probe_media(tools, &temp_output)?;
    let tolerance = (inputs.len() as f64 * 0.01).clamp(0.25, 2.0);
    if (merged.duration_seconds - expected_duration).abs() > tolerance {
        return Err(AppError::new("MERGE_VALIDATION_FAILED", "合并输出验证失败"));
    }
    check_cancelled(cancellation)?;
    if request.conflict_policy == MergeConflictPolicy::Overwrite && output_path.exists() {
        fs::remove_file(&output_path).map_err(output_invalid)?;
    }
    fs::rename(&temp_output, &output_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            AppError::new("MERGE_OUTPUT_EXISTS", "合并视频已存在，不能重复合并")
        } else {
            output_invalid(error)
        }
    })?;
    Ok(MergeResult { output_path })
}

fn parse_probe_json(bytes: &[u8]) -> Result<MediaProbe, AppError> {
    let raw: RawProbe = serde_json::from_slice(bytes).map_err(|_| invalid_probe())?;
    let videos = raw
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("video"))
        .collect::<Vec<_>>();
    let audios = raw
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("audio"))
        .collect::<Vec<_>>();
    if videos.len() != 1 || audios.len() > 1 {
        return Err(invalid_probe());
    }
    let video = videos[0];
    let duration_seconds = raw
        .format
        .duration
        .as_deref()
        .ok_or_else(invalid_probe)?
        .parse::<f64>()
        .map_err(|_| invalid_probe())?;
    if !duration_seconds.is_finite() || duration_seconds < 0.0 {
        return Err(invalid_probe());
    }
    Ok(MediaProbe {
        video: StreamSignature {
            codec_name: video.codec_name.clone().ok_or_else(invalid_probe)?,
            width: video.width,
            height: video.height,
            frame_rate: video.r_frame_rate.clone(),
            time_base: video.time_base.clone(),
            sample_rate: None,
            channels: None,
        },
        audio: audios
            .first()
            .map(|audio| {
                Ok(StreamSignature {
                    codec_name: audio.codec_name.clone().ok_or_else(invalid_probe)?,
                    width: None,
                    height: None,
                    frame_rate: None,
                    time_base: audio.time_base.clone(),
                    sample_rate: audio
                        .sample_rate
                        .as_deref()
                        .map(str::parse)
                        .transpose()
                        .map_err(|_| invalid_probe())?,
                    channels: audio.channels,
                })
            })
            .transpose()?,
        duration_seconds,
    })
}

fn validate_inputs(root: &Path, inputs: &[MergeInput]) -> Result<(), AppError> {
    let mut previous = None;
    for input in inputs {
        if input.episode_index == 0 || previous == Some(input.episode_index) {
            return Err(AppError::new(
                "MERGE_INPUT_INVALID",
                "合并集数必须为不重复的正整数",
            ));
        }
        previous = Some(input.episode_index);
        let path = fs::canonicalize(&input.path).map_err(input_changed)?;
        let metadata = fs::symlink_metadata(&path).map_err(input_changed)?;
        let modified = metadata
            .modified()
            .and_then(|value| {
                value
                    .duration_since(UNIX_EPOCH)
                    .map_err(std::io::Error::other)
            })
            .map(|value| value.as_nanos())
            .map_err(input_changed)?;
        if !path.starts_with(root)
            || !metadata.is_file()
            || is_reparse_point(&metadata)
            || metadata.file_size() != input.size
            || modified != input.modified_unix_nanos
        {
            return Err(input_changed("input snapshot mismatch"));
        }
    }
    Ok(())
}

fn ensure_real_directory(path: &Path) -> Result<(), AppError> {
    let metadata = fs::symlink_metadata(path).map_err(output_invalid)?;
    if !metadata.is_dir() || is_reparse_point(&metadata) {
        return Err(output_invalid("directory is a reparse point"));
    }
    Ok(())
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn validate_output_name(name: &str) -> Result<(), AppError> {
    let path = Path::new(name);
    let mut components = path.components();
    if name.is_empty()
        || name.contains(['\0', '/', '\\'])
        || path.is_absolute()
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || path.extension().and_then(|value| value.to_str()) != Some("mp4")
    {
        return Err(output_invalid("output name is invalid"));
    }
    Ok(())
}

fn write_concat_file(path: &Path, inputs: &[MergeInput]) -> Result<(), AppError> {
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(output_invalid)?;
    file.write_all(b"ffconcat version 1.0\n")
        .map_err(output_invalid)?;
    for input in inputs {
        let value = input
            .path
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "'\\''");
        writeln!(file, "file '{value}'").map_err(output_invalid)?;
    }
    file.sync_all().map_err(output_invalid)
}

fn run_ffmpeg(
    tools: &MediaTools,
    concat: &Path,
    output: &Path,
    encoder: VideoEncoder,
    quality: MergeQuality,
    duration: f64,
    cancellation: &CancellationToken,
    progress: &mut dyn FnMut(MergeProgress),
) -> Result<ProcessOutcome, AppError> {
    let mut command = tools.ffmpeg_command();
    command.args(["-hide_banner", "-y", "-f", "concat", "-safe", "0", "-i"]);
    command
        .arg(concat)
        .args(["-map", "0:v:0", "-map", "0:a:0?"]);
    match encoder {
        VideoEncoder::Copy => {
            command.args(["-c", "copy"]);
        }
        VideoEncoder::Nvenc => {
            command.args(nvenc_args(quality));
        }
        VideoEncoder::Libx264 => {
            command.args(libx264_args(quality));
        }
        VideoEncoder::VideoToolbox => {
            command.args(["-c:v", "h264_videotoolbox", "-c:a", "aac"]);
        }
    }
    command
        .args(["-movflags", "+faststart", "-progress", "pipe:1", "-nostats"])
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cancellation.prepare_command(&mut command);
    let mut child = command.spawn().map_err(|error| {
        AppError::with_cause("FFMPEG_FAILED", "无法运行打包的合并工具", error.to_string())
    })?;
    cancellation.register_child(&mut child)?;
    collect_process(child, duration, cancellation, progress)
}

fn collect_process(
    mut child: Child,
    duration: f64,
    cancellation: &CancellationToken,
    progress: &mut dyn FnMut(MergeProgress),
) -> Result<ProcessOutcome, AppError> {
    let child_id = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ffmpeg_error("missing stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| ffmpeg_error("missing stderr"))?;
    let (sender, receiver) = mpsc::channel();
    let output_thread = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = sender.send(line);
        }
    });
    let error_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let started = Instant::now();
    let status = loop {
        for line in receiver.try_iter() {
            if let Some(value) = line
                .strip_prefix("out_time_us=")
                .and_then(|v| v.parse::<f64>().ok())
            {
                let percent = if duration > 0.0 {
                    (value / 1_000_000.0 / duration * 100.0).clamp(0.0, 99.0)
                } else {
                    0.0
                };
                progress(event("merging", percent, false));
            }
        }
        if cancellation.is_cancelled() {
            cancellation.kill();
            let _ = child.kill();
            let _ = child.wait();
            cancellation.clear_child(child_id);
            return Err(cancelled_error());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                let _ = child.kill();
                cancellation.clear_child(child_id);
                return Err(ffmpeg_error(error));
            }
        }
        if started.elapsed() > Duration::from_secs(7 * 24 * 60 * 60) {
            let _ = child.kill();
            cancellation.clear_child(child_id);
            return Err(ffmpeg_error("merge exceeded safety timeout"));
        }
    };
    cancellation.clear_child(child_id);
    let _ = output_thread.join();
    Ok(ProcessOutcome {
        success: status.success(),
        stderr: error_thread.join().unwrap_or_default(),
    })
}

fn nvenc_args(quality: MergeQuality) -> [&'static str; 12] {
    let (preset, cq) = match quality {
        MergeQuality::High => ("p6", "19"),
        MergeQuality::Balanced => ("p5", "23"),
        MergeQuality::Compact => ("p4", "28"),
    };
    [
        "-c:v",
        "h264_nvenc",
        "-preset",
        preset,
        "-tune",
        "hq",
        "-rc",
        "vbr",
        "-cq",
        cq,
        "-c:a",
        "aac",
    ]
}

fn libx264_args(quality: MergeQuality) -> [&'static str; 8] {
    let crf = match quality {
        MergeQuality::High => "18",
        MergeQuality::Balanced => "23",
        MergeQuality::Compact => "28",
    };
    [
        "-c:v", "libx264", "-preset", "fast", "-crf", crf, "-c:a", "aac",
    ]
}

fn nvenc_runtime_failure(stderr: &str) -> bool {
    let value = stderr.to_ascii_lowercase();
    [
        "cannot load nvcuda",
        "no nvenc capable devices",
        "driver does not support",
        "openencode session",
        "initialize the encoder",
        "unsupported device",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

fn encoder_stage(encoder: VideoEncoder) -> &'static str {
    match encoder {
        VideoEncoder::Copy => "正在复制视频流",
        VideoEncoder::Nvenc => "正在使用 NVIDIA GPU 编码",
        VideoEncoder::VideoToolbox => "正在使用系统 GPU 编码",
        VideoEncoder::Libx264 => "正在使用 CPU 编码",
    }
}

struct ProcessOutcome {
    success: bool,
    stderr: String,
}

struct TempDirectory {
    path: PathBuf,
}

impl TempDirectory {
    fn create(root: &Path) -> Result<Self, AppError> {
        let random = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = root.join(format!(
            ".hongguo-merge-{}-{random:032x}",
            std::process::id()
        ));
        fs::create_dir(&path).map_err(output_invalid)?;
        ensure_real_directory(&path)?;
        Ok(Self { path })
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn event(stage: &str, percent: f64, terminal: bool) -> MergeProgress {
    MergeProgress {
        stage: stage.into(),
        percent,
        terminal,
    }
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), AppError> {
    if cancellation.is_cancelled() {
        Err(cancelled_error())
    } else {
        Ok(())
    }
}

fn invalid_probe() -> AppError {
    AppError::new("FFPROBE_INVALID", "媒体检测结果无效")
}
fn cancelled_error() -> AppError {
    AppError::new("MERGE_CANCELLED", "合并任务已取消")
}
fn input_changed(cause: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "MEDIA_INPUT_CHANGED",
        "合并输入已移动或发生变化",
        cause.to_string(),
    )
}
fn output_invalid(cause: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "MEDIA_OUTPUT_INVALID",
        "合并输出必须位于已选剧目目录内",
        cause.to_string(),
    )
}
fn ffmpeg_error(cause: impl std::fmt::Display) -> AppError {
    AppError::with_cause("FFMPEG_FAILED", "视频合并失败", cause.to_string())
}
