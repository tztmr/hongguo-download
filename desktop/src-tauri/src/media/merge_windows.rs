use super::{
    hardware::{probe_video_hardware, select_video_encoder, VideoEncoder},
    model::{MergeConflictPolicy, MergeInput, MergeMode, MergeQuality, MergeRequest},
    process_control::CancellationToken,
    tools::MediaTools,
};
use crate::AppError;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    process::{Child, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

pub use super::probe::MediaProbe;

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
    Ok(probe_details(tools, path)?.media)
}

struct ProbeDetails {
    media: MediaProbe,
    video_config: Vec<serde_json::Value>,
    audio_config: Option<Vec<serde_json::Value>>,
    video_frames: Option<u64>,
    video_start: f64,
    video_duration: f64,
    audio_start: f64,
}

fn probe_details(tools: &MediaTools, path: &Path) -> Result<ProbeDetails, AppError> {
    probe_details_inner(tools, path).map_err(|error| probe_context(error, path))
}

fn probe_context(mut error: AppError, path: &Path) -> AppError {
    let file = path.file_name().unwrap_or_default().to_string_lossy();
    let size = fs::metadata(path)
        .map(|m| format!("{} 字节", m.len()))
        .unwrap_or_else(|_| "文件不可访问".into());
    error.message = format!("{}（文件：{file}，{size}）", error.message);
    error
}

fn probe_details_inner(tools: &MediaTools, path: &Path) -> Result<ProbeDetails, AppError> {
    let output = tools
        .ffprobe_command()
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
            "-show_data_hash",
            "sha256",
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
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        let reason = if stderr.contains("no such file") || stderr.contains("permission denied") {
            "媒体文件无法访问，请检查文件路径与读取权限"
        } else if stderr.contains("moov atom not found")
            || stderr.contains("invalid data")
            || stderr.contains("end of file")
        {
            "媒体文件不完整或格式无效"
        } else {
            "媒体文件无法读取"
        };
        return Err(AppError::with_cause("FFPROBE_FAILED", reason, stderr));
    }
    let raw = super::probe::decode(&output.stdout)?;
    let media = super::probe::parse_value(&raw)?;
    let streams = raw["streams"].as_array().ok_or_else(invalid_probe)?;
    let video = streams
        .iter()
        .find(|s| super::probe::is_video(s))
        .ok_or_else(invalid_probe)?;
    let number =
        |value: &serde_json::Value, fallback: f64| super::probe::number(value).unwrap_or(fallback);
    let audio = streams.iter().find(|s| s["codec_type"] == "audio");
    Ok(ProbeDetails {
        video_config: [
            "extradata_hash",
            "profile",
            "level",
            "pix_fmt",
            "sample_aspect_ratio",
            "color_range",
            "color_space",
            "color_transfer",
            "color_primaries",
        ]
        .iter()
        .map(|key| video[key].clone())
        .collect(),
        audio_config: audio.map(|a| {
            ["extradata_hash", "profile", "channel_layout"]
                .iter()
                .map(|key| a[key].clone())
                .collect()
        }),
        video_frames: video["nb_frames"]
            .as_str()
            .and_then(|value| value.parse().ok()),
        video_start: number(&video["start_time"], f64::NAN),
        video_duration: super::probe::stream_duration(video).unwrap_or(f64::NAN),
        audio_start: audio.map(|a| number(&a["start_time"], 0.0)).unwrap_or(0.0),
        media,
    })
}

fn can_copy_video(probes: &[ProbeDetails]) -> bool {
    let Some(first) = probes.first() else {
        return false;
    };
    // Audio normalisation is safe only when each episode shares decoder config
    // and starts on the same zero-based video timeline.
    matches!(first.media.video.codec_name.as_str(), "h264" | "hevc")
        && first
            .video_config
            .first()
            .is_some_and(|v| v.as_str().is_some())
        && probes.iter().all(|p| {
            let mut video = p.media.video.clone();
            video.time_base = first.media.video.time_base.clone();
            video == first.media.video
                && p.video_config == first.video_config
                && p.video_start.abs() <= 0.002
                && p.video_duration.is_finite()
                && p.video_duration > 0.0
        })
}

pub fn run_merge<F>(
    tools: &MediaTools,
    request: MergeRequest,
    cancellation: &CancellationToken,
    progress: F,
) -> Result<MergeResult, AppError>
where
    F: FnMut(MergeProgress),
{
    run_merge_with_budget(tools, request, cancellation, None, progress)
}

pub(super) fn run_merge_with_budget<F>(
    tools: &MediaTools,
    request: MergeRequest,
    cancellation: &CancellationToken,
    budget: Option<super::scheduling::ExecutionBudget>,
    mut progress: F,
) -> Result<MergeResult, AppError>
where
    F: FnMut(MergeProgress),
{
    progress(event("probing", 0.0, false));
    let result = run_merge_inner(tools, request, cancellation, budget, &mut progress);
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
    budget: Option<super::scheduling::ExecutionBudget>,
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
    let mut details = Vec::with_capacity(inputs.len());
    // Probes only read a small amount of metadata. Bound parallelism so hundreds
    // of episodes don't pay one Windows process-start delay at a time.
    for batch in inputs.chunks(4) {
        check_cancelled(cancellation)?;
        let probed = thread::scope(|scope| {
            let readers = batch
                .iter()
                .map(|input| scope.spawn(move || probe_details(tools, &input.path)))
                .collect::<Vec<_>>();
            readers
                .into_iter()
                .map(|reader| reader.join().map_err(|_| invalid_probe())?)
                .collect::<Result<Vec<_>, AppError>>()
        })?;
        details.extend(probed);
        progress(event(
            &format!("正在检测媒体参数 {}/{}", details.len(), inputs.len()),
            0.0,
            false,
        ));
    }
    validate_inputs(&root, &inputs)?;
    let probes = details.iter().map(|p| p.media.clone()).collect::<Vec<_>>();
    let compatible = can_stream_copy(&probes)
        && details.iter().all(|p| {
            p.video_config == details[0].video_config && p.audio_config == details[0].audio_config
        });
    let audio_only = !request.square_canvas
        && request.mode == Some(MergeMode::Auto)
        && !compatible
        && can_copy_video(&details);
    let needs_transcode = request.square_canvas
        || match request.mode {
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
            Some(MergeMode::Auto) => !compatible && !audio_only,
            None => request.transcode_h264,
        };
    if !needs_transcode && !compatible && !audio_only {
        return Err(AppError::new(
            "MERGE_TRANSCODE_REQUIRED",
            "输入媒体参数不一致，需要开启 H.264 转码",
        ));
    }

    let temp_output = temp.path.join("merged.mp4");
    let normalize = audio_only || needs_transcode;
    let has_audio = probes.iter().any(|p| p.audio.is_some());
    let expected_duration = if normalize {
        let encoder = if needs_transcode {
            if budget.is_some_and(|b| b.force_cpu) {
                progress(event("已分配 CPU 合并通道，使用空闲资源转码", 0.0, false));
                VideoEncoder::Libx264
            } else {
                let hardware = probe_video_hardware(tools);
                select_video_encoder(true, hardware.nvenc_available)
            }
        } else {
            VideoEncoder::Copy
        };
        let mut spec = NormalizeSpec::new(&details, encoder, request.quality)
            .map_err(|error| probe_context(error, &inputs[0].path))?;
        if request.square_canvas {
            spec.square_canvas = true;
            spec.width = super::framing::square_side(spec.width, spec.height);
            spec.height = spec.width;
            progress(event("首集转换为 1:1 方形，保留完整画面", 0.0, false));
        }
        if let Some(budget) = budget {
            spec.cpu_threads = budget.cpu_threads.max(1);
        }
        match normalize_inputs(
            tools,
            &inputs,
            &details,
            &temp.path,
            &spec,
            cancellation,
            progress,
        ) {
            Ok(duration) => duration,
            Err(error) if error.code == "MERGE_NVENC_RETRY" => {
                progress(event("NVENC 不可用，改用 CPU 重新编码", 0.0, false));
                let spec = NormalizeSpec {
                    encoder: VideoEncoder::Libx264,
                    ..spec
                };
                normalize_inputs(
                    tools,
                    &inputs,
                    &details,
                    &temp.path,
                    &spec,
                    cancellation,
                    progress,
                )?
            }
            Err(error) => return Err(error),
        }
    } else {
        write_concat_file(&concat, &inputs)?;
        probes.iter().map(|p| p.duration_seconds).sum::<f64>()
    };
    let mut final_progress = |mut update: MergeProgress| {
        if normalize {
            update.stage = if has_audio {
                "正在无损合并画面并编码音频"
            } else {
                "正在无损合并画面"
            }
            .into();
            update.percent = 85.0 + update.percent * 0.14;
        }
        progress(update);
    };
    final_progress(event(encoder_stage(VideoEncoder::Copy), 0.0, false));
    let outcome = run_ffmpeg(
        tools,
        &concat,
        &temp_output,
        normalize && has_audio,
        expected_duration,
        cancellation,
        &mut final_progress,
    )?;
    if !outcome.success {
        return Err(ffmpeg_error(outcome.stderr));
    }

    let checked_output = probe_details(tools, &temp_output)?;
    let merged = &checked_output.media;
    if request.square_canvas && merged.video.width != merged.video.height {
        return Err(AppError::new(
            "MERGE_VALIDATION_FAILED",
            "首集方形画幅校验失败，保留源视频",
        ));
    }
    if !needs_transcode {
        let expected_frames = details
            .iter()
            .map(|p| p.video_frames)
            .collect::<Option<Vec<_>>>();
        if let (Some(frames), Some(actual)) = (expected_frames, checked_output.video_frames) {
            let expected = frames.iter().sum::<u64>();
            if expected != actual {
                return Err(AppError::new(
                    "MERGE_VALIDATION_FAILED",
                    format!("合并输出验证失败：预期 {expected} 帧，实际 {actual} 帧"),
                ));
            }
        }
    }
    let tolerance = (inputs.len() as f64 * 0.01).clamp(0.25, 2.0);
    if (merged.duration_seconds - expected_duration).abs() > tolerance {
        return Err(AppError::new(
            "MERGE_VALIDATION_FAILED",
            format!(
                "合并输出验证失败：预期 {:.3} 秒，实际 {:.3} 秒，差 {:.3} 秒（{} 集）",
                expected_duration,
                merged.duration_seconds,
                (merged.duration_seconds - expected_duration).abs(),
                inputs.len()
            ),
        ));
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
            || metadata.len() != input.size
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
    #[cfg(windows)]
    {
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
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
        writeln!(file, "{}", concat_entry(&input.path)?).map_err(output_invalid)?;
    }
    file.sync_all().map_err(output_invalid)
}

fn concat_entry(path: &Path) -> Result<String, AppError> {
    let value = path
        .to_str()
        .ok_or_else(|| input_changed("non-Unicode path"))?;
    if value.chars().any(char::is_control) {
        return Err(input_changed("control character in concat path"));
    }
    // Explicit file: avoids URL resolution against concat.txt. In particular,
    // keep the native \\?\ prefix intact; //?/ is parsed as a URL query.
    // Backslashes are literal inside FFmpeg's single quotes; escape only '.
    Ok(format!("file 'file:{}'", value.replace('\'', "'\\''")))
}

struct NormalizeSpec {
    square_canvas: bool,
    cpu_threads: usize,
    encoder: VideoEncoder,
    quality: MergeQuality,
    audio: bool,
    width: u32,
    height: u32,
    rate: String,
    fps: f64,
}

impl NormalizeSpec {
    fn new(
        probes: &[ProbeDetails],
        encoder: VideoEncoder,
        quality: MergeQuality,
    ) -> Result<Self, AppError> {
        let video = &probes[0].media.video;
        let rate = video
            .frame_rate
            .clone()
            .ok_or_else(|| AppError::new("FFPROBE_INVALID", "媒体检测失败：缺少可用视频帧率"))?;
        let fps = super::probe::ratio(&rate)
            .filter(|fps| (1.0..=120.0).contains(fps))
            .ok_or_else(|| {
                AppError::new(
                    "FFPROBE_INVALID",
                    format!("媒体检测失败：视频帧率 {rate} 超出可处理范围"),
                )
            })?;
        let dimensions_error = || {
            AppError::new(
                "FFPROBE_INVALID",
                format!(
                    "媒体检测失败：视频分辨率 {}×{} 超出可处理范围",
                    video.width.unwrap_or(0),
                    video.height.unwrap_or(0)
                ),
            )
        };
        Ok(Self {
            square_canvas: false,
            cpu_threads: 0,
            encoder,
            quality,
            audio: probes.iter().any(|p| p.media.audio.is_some()),
            width: video
                .width
                .filter(|v| (2..=8192).contains(v))
                .ok_or_else(dimensions_error)?
                & !1,
            height: video
                .height
                .filter(|v| (2..=8192).contains(v))
                .ok_or_else(dimensions_error)?
                & !1,
            rate,
            fps,
        })
    }
    fn duration(&self, probe: &ProbeDetails) -> f64 {
        let duration = if probe.video_duration.is_finite() && probe.video_duration > 0.0 {
            probe.video_duration
        } else {
            probe.media.duration_seconds
        };
        if self.encoder == VideoEncoder::Copy {
            duration
        } else {
            (duration * self.fps).round().max(1.0) / self.fps
        }
    }
}

fn normalize_inputs(
    tools: &MediaTools,
    inputs: &[MergeInput],
    probes: &[ProbeDetails],
    directory: &Path,
    spec: &NormalizeSpec,
    cancellation: &CancellationToken,
    progress: &mut dyn FnMut(MergeProgress),
) -> Result<f64, AppError> {
    let durations = probes.iter().map(|p| spec.duration(p)).collect::<Vec<_>>();
    let total = durations.iter().sum::<f64>();
    let mut completed = 0.0;
    let mut normalized = Vec::new();
    let mut cuda_decode = spec.encoder == VideoEncoder::Nvenc;
    for (index, (input, probe)) in inputs.iter().zip(probes).enumerate() {
        check_cancelled(cancellation)?;
        let path = directory.join(format!("segment-{index:05}.mov"));
        let duration = durations[index];
        let stage = format!(
            "{} {}/{}",
            if spec.encoder == VideoEncoder::Copy {
                "保留原画面，统一音频与时间轴"
            } else {
                encoder_stage(spec.encoder)
            },
            index + 1,
            inputs.len()
        );
        let mut highest_percent: f64 = 0.0;
        let mut update = |mut event: MergeProgress| {
            highest_percent = highest_percent.max(event.percent);
            event.percent = (completed + duration * highest_percent / 100.0) / total * 85.0;
            progress(event);
        };
        update(event(&stage, 0.0, false));
        let result = loop {
            let mut command = tools.ffmpeg_command();
            command.args(["-hide_banner", "-y"]);
            if spec.cpu_threads > 0 {
                command.args([
                    "-filter_threads",
                    &spec.cpu_threads.to_string(),
                    "-threads",
                    &spec.cpu_threads.to_string(),
                ]);
            }
            if cuda_decode {
                // Keep decoded frames in system memory for the existing timing,
                // scale and pad filters; NVDEC and NVENC work alongside CPU filters.
                command.args(["-hwaccel", "cuda"]);
            }
            command.arg("-i").arg(&input.path);
            let audio = probe.media.audio.is_some();
            if spec.audio && !audio {
                command.args(["-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo"]);
            }
            command.args(["-map", "0:V:0"]);
            if spec.audio {
                let start = if probe.video_start.is_finite() {
                    probe.video_start
                } else {
                    0.0
                };
                let offset = if audio {
                    probe.audio_start - start
                } else {
                    0.0
                };
                command.args(["-map", if audio { "0:a:0" } else { "1:a:0" }, "-af"])
                .arg(format!("asetpts=PTS-STARTPTS+{offset:.9}/TB,aresample=48000:async=1:first_pts=0,apad,atrim=duration={duration:.9}"));
            }
            match spec.encoder {
                VideoEncoder::Copy => {
                    command.args(["-c:v", "copy"]);
                }
                encoder => {
                    let framing = if spec.square_canvas {
                        super::framing::square_filter(spec.width)
                    } else {
                        format!("scale={}:{}:force_original_aspect_ratio=decrease:force_divisible_by=2,pad={}:{}:(ow-iw)/2:(oh-ih)/2,setsar=1", spec.width, spec.height, spec.width, spec.height)
                    };
                    command
                        .arg("-vf")
                        .arg(format!("setpts=PTS-STARTPTS,fps={},{}", spec.rate, framing));
                    match encoder {
                        VideoEncoder::Nvenc => {
                            command.args(nvenc_args(spec.quality));
                        }
                        VideoEncoder::VideoToolbox => {
                            command.args(["-c:v", "h264_videotoolbox"]);
                        }
                        _ => {
                            command.args(libx264_args(spec.quality));
                        }
                    }
                    command.args(["-pix_fmt", "yuv420p"]);
                }
            }
            // PCM intermediates avoid accumulating AAC priming at each episode join.
            if spec.cpu_threads > 0 {
                command.args(["-threads", &spec.cpu_threads.to_string()]);
            }
            if spec.audio {
                command.args(["-c:a", "pcm_s16le", "-ar", "48000", "-ac", "2"]);
            } else {
                command.arg("-an");
            }
            command
                .args([
                    "-t",
                    &format!("{duration:.9}"),
                    "-video_track_timescale",
                    "90000",
                    "-progress",
                    "pipe:1",
                    "-nostats",
                ])
                .arg(&path)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let result = run_command(command, duration, &stage, cancellation, &mut update)?;
            if !result.success && cuda_decode && cuda_decode_failure(&result.stderr) {
                check_cancelled(cancellation)?;
                cuda_decode = false;
                update(event(
                    "GPU 解码不可用，改用 CPU 解码并保留 NVIDIA GPU 编码",
                    0.0,
                    false,
                ));
                continue;
            }
            break result;
        };
        if !result.success {
            if spec.encoder == VideoEncoder::Nvenc && nvenc_runtime_failure(&result.stderr) {
                return Err(AppError::new("MERGE_NVENC_RETRY", "NVENC 不可用"));
            }
            return Err(ffmpeg_error(result.stderr));
        }
        completed += duration;
        normalized.push(MergeInput {
            path,
            ..input.clone()
        });
    }
    write_concat_file(&directory.join("concat.txt"), &normalized)?;
    Ok(total)
}

fn run_ffmpeg(
    tools: &MediaTools,
    concat: &Path,
    output: &Path,
    audio_only: bool,
    duration: f64,
    cancellation: &CancellationToken,
    progress: &mut dyn FnMut(MergeProgress),
) -> Result<ProcessOutcome, AppError> {
    let mut command = tools.ffmpeg_command();
    command.args(["-hide_banner", "-y", "-f", "concat", "-safe", "0", "-i"]);
    command
        .arg(concat)
        .args(["-map", "0:V:0", "-map", "0:a:0?"]);
    command.args(["-c", "copy"]);
    if audio_only {
        command.args(["-c:a", "aac", "-b:a", "192k", "-ar", "48000", "-ac", "2"]);
    }
    command
        .args(["-movflags", "+faststart", "-progress", "pipe:1", "-nostats"])
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run_command(
        command,
        duration,
        encoder_stage(VideoEncoder::Copy),
        cancellation,
        progress,
    )
}

fn run_command(
    mut command: std::process::Command,
    duration: f64,
    stage: &str,
    cancellation: &CancellationToken,
    progress: &mut dyn FnMut(MergeProgress),
) -> Result<ProcessOutcome, AppError> {
    cancellation.prepare_command(&mut command);
    let mut child = command.spawn().map_err(|error| {
        AppError::with_cause("FFMPEG_FAILED", "无法运行打包的合并工具", error.to_string())
    })?;
    if let Err(error) = cancellation.register_child(&mut child) {
        let _ = child.kill();
        let _ = child.wait();
        cancellation.clear_child(child.id());
        return Err(error);
    }
    collect_process(child, duration, stage, cancellation, progress)
}

fn collect_process(
    mut child: Child,
    duration: f64,
    stage: &str,
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
                progress(event(stage, percent, false));
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
        "cannot load nvencodeapi",
        "initializeencoder failed",
        "openencodesessionex failed",
        "no capable devices found",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

fn cuda_decode_failure(stderr: &str) -> bool {
    let value = stderr.to_ascii_lowercase();
    [
        "cuda_error",
        "cannot load nvcuda",
        "failed setup for format cuda",
        "hwaccel initialisation",
        "hardware device setup failed",
        "device setup failed for decoder",
        "no device available for decoder",
        "cuvid",
        "cannot load libnvcuvid",
    ]
    .iter()
    .any(|token| value.contains(token))
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

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
            ".hongguo-merge-{}-{random:032x}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
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

#[cfg(test)]
mod performance_tests {
    use super::*;

    #[test]
    fn concat_preserves_windows_extended_drive_and_unc_paths() {
        for value in [
            r"D:\中文 剧集\0001.mp4",
            r"\\?\D:\中文 剧集\0001.mp4",
            r"\\?\UNC\server\视频\0001.mp4",
        ] {
            assert_eq!(
                concat_entry(Path::new(value)).unwrap(),
                format!("file 'file:{value}'")
            );
        }
        assert!(concat_entry(Path::new("D:/bad\nfile.mp4")).is_err());
    }

    #[test]
    fn smart_merge_reads_unicode_long_paths_and_apostrophes() {
        let Some(tools) = tools() else {
            return;
        };
        let temp = TempDirectory::create(&std::env::temp_dir()).unwrap();
        let mut root = fs::canonicalize(&temp.path).unwrap();
        for _ in 0..5 {
            root = root.join("中文 长路径 剧集目录 abcdefghijklmnopqrstuvwxyz");
        }
        fs::create_dir_all(&root).unwrap();
        root = fs::canonicalize(root).unwrap();
        let source = root.join("第 01 集 It's a story.mp4");
        let result = tools
            .ffmpeg_command()
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=128x192:rate=12",
                "-t",
                "1",
                "-an",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let source = fs::canonicalize(source).unwrap();
        assert!(source.to_string_lossy().encode_utf16().count() > 260);
        crate::media::download_validation::validate_mp4(&source).unwrap();
        let result = run_merge(
            &tools,
            MergeRequest {
                series_root: root,
                output_file_name: "中文 合并.mp4".into(),
                inputs: vec![snapshot(source.clone(), 1), snapshot(source.clone(), 2)],
                transcode_h264: false,
                square_canvas: false,
                mode: Some(MergeMode::Auto),
                quality: MergeQuality::High,
                conflict_policy: MergeConflictPolicy::FailIfExists,
            },
            &CancellationToken::default(),
            |_| {},
        )
        .unwrap();
        assert!(
            (probe_media(&tools, &result.output_path)
                .unwrap()
                .duration_seconds
                - 2.0)
                .abs()
                < 0.03
        );
        assert_eq!(
            frame_hashes(&tools, &result.output_path),
            (0..2)
                .flat_map(|_| frame_hashes(&tools, &source))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn decoder_failure_is_distinct_from_io_failure() {
        assert!(cuda_decode_failure(
            "Device setup failed for decoder: CUDA_ERROR_NOT_SUPPORTED"
        ));
        assert!(cuda_decode_failure(
            "Failed setup for format cuda: hwaccel initialisation returned error"
        ));
        assert!(!cuda_decode_failure("No space left on device"));
        assert!(!cuda_decode_failure(
            "Error opening output: Permission denied"
        ));
    }

    fn tools() -> Option<MediaTools> {
        let directory = std::env::var_os("HONGGUO_TEST_PLAYBACK_TOOLS")?;
        let suffix = if cfg!(windows) { ".exe" } else { "" };
        let root = PathBuf::from(directory);
        Some(MediaTools::from_test_paths(
            root.join(format!("ffmpeg{suffix}")),
            root.join(format!("ffprobe{suffix}")),
        ))
    }

    fn snapshot(path: PathBuf, episode_index: u32) -> MergeInput {
        let meta = fs::metadata(&path).unwrap();
        MergeInput {
            path,
            episode_index,
            size: meta.len(),
            modified_unix_nanos: meta
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        }
    }

    fn frame_hashes(tools: &MediaTools, path: &Path) -> Vec<String> {
        let out = tools
            .ffmpeg_command()
            .args(["-v", "error", "-i"])
            .arg(path)
            .args(["-map", "0:V:0", "-f", "framemd5", "-"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| l.rsplit(',').next().unwrap().trim().into())
            .collect()
    }

    #[test]
    fn differing_time_bases_do_not_stretch_video_or_trigger_gpu_transcoding() {
        let Some(tools) = tools() else {
            return;
        };
        let temp = TempDirectory::create(&std::env::temp_dir()).unwrap();
        let source = temp.path.join("base.mp4");
        let second = temp.path.join("different-timebase.mp4");
        assert!(tools
            .ffmpeg_command()
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=128x192:rate=12",
                "-t",
                "1",
                "-an",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p"
            ])
            .arg(&source)
            .status()
            .unwrap()
            .success());
        assert!(tools
            .ffmpeg_command()
            .args(["-v", "error", "-i"])
            .arg(&source)
            .args(["-c", "copy", "-video_track_timescale", "90000"])
            .arg(&second)
            .status()
            .unwrap()
            .success());
        let request = MergeRequest {
            series_root: temp.path.clone(),
            output_file_name: "时长验证.mp4".into(),
            inputs: vec![snapshot(source.clone(), 1), snapshot(second, 2)],
            transcode_h264: false,
            square_canvas: false,
            mode: Some(MergeMode::Auto),
            quality: MergeQuality::High,
            conflict_policy: MergeConflictPolicy::FailIfExists,
        };
        let mut stages = Vec::new();
        let result = run_merge(&tools, request, &CancellationToken::default(), |p| {
            stages.push(p.stage)
        })
        .unwrap();
        assert!(!stages
            .iter()
            .any(|s| s.contains("GPU") || s.contains("CPU")));
        assert!(
            (probe_media(&tools, &result.output_path)
                .unwrap()
                .duration_seconds
                - 2.0)
                .abs()
                < 0.03
        );
        assert_eq!(
            frame_hashes(&tools, &result.output_path),
            (0..2)
                .flat_map(|_| frame_hashes(&tools, &source))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn square_shorts_preserve_full_frame_audio_and_duration() {
        let Some(tools) = tools() else {
            return;
        };
        let temp = TempDirectory::create(&std::env::temp_dir()).unwrap();
        for (name, dimensions, sar, side, bounds) in [
            ("landscape", "320x180", "1", 320usize, (0, 70, 320, 180)),
            ("portrait", "180x320", "1", 320, (70, 0, 180, 320)),
            ("square", "240x240", "1", 240, (0, 0, 240, 240)),
            ("anamorphic", "180x180", "2", 180, (0, 44, 180, 90)),
            ("rotated", "320x180", "1", 320, (70, 0, 180, 320)),
        ] {
            let source = temp.path.join(format!("{name}.mp4"));
            let out = tools
                .ffmpeg_command()
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("color=white:size={dimensions}:rate=12"),
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=48000",
                    "-t",
                    "1",
                    "-vf",
                    &format!("setsar={sar}"),
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "aac",
                ])
                .arg(&source)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            if name == "rotated" {
                let rotated = temp.path.join("with-rotation.mp4");
                let out = tools
                    .ffmpeg_command()
                    .args(["-v", "error", "-display_rotation", "90", "-i"])
                    .arg(&source)
                    .args(["-c", "copy"])
                    .arg(&rotated)
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
                fs::copy(rotated, &source).unwrap();
                let raw = tools
                    .ffprobe_command()
                    .args(["-v", "error", "-show_streams", "-of", "json"])
                    .arg(&source)
                    .output()
                    .unwrap();
                let probe: serde_json::Value = serde_json::from_slice(&raw.stdout).unwrap();
                assert!(
                    probe["streams"][0]["side_data_list"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|s| s["rotation"] == 90),
                    "fixture must contain a rotation matrix"
                );
            }
            let original = fs::read(&source).unwrap();
            // Deserialize the public request, also exercising compatibility of the new option.
            let request: MergeRequest = serde_json::from_value(serde_json::json!({
                "seriesRoot": temp.path, "outputFileName": format!("{name}-short.mp4"),
                "inputs": [snapshot(source.clone(), 1)], "mode": "auto", "squareCanvas": true
            }))
            .unwrap();
            let result = run_merge_with_budget(
                &tools,
                request,
                &CancellationToken::default(),
                Some(super::super::scheduling::ExecutionBudget {
                    cpu_threads: 2,
                    force_cpu: true,
                    merge: true,
                    memory: 0,
                    gpu_memory: 0,
                }),
                |_| {},
            )
            .unwrap();
            let probe = probe_media(&tools, &result.output_path).unwrap();
            assert_eq!(
                (probe.video.width, probe.video.height),
                (Some(side as u32), Some(side as u32)),
                "{name}"
            );
            assert!((probe.duration_seconds - 1.0).abs() < 0.1, "{name}");
            assert!(probe.audio.is_some(), "{name}: audio lost");
            assert_eq!(
                fs::read(&source).unwrap(),
                original,
                "source must remain intact"
            );
            let frame = tools
                .ffmpeg_command()
                .args(["-v", "error", "-i"])
                .arg(&result.output_path)
                .args([
                    "-frames:v",
                    "1",
                    "-pix_fmt",
                    "rgb24",
                    "-f",
                    "rawvideo",
                    "pipe:1",
                ])
                .output()
                .unwrap();
            assert!(frame.status.success());
            assert_eq!(frame.stdout.len(), side * side * 3);
            let (x, y, w, h) = bounds;
            for row in 0..side {
                for col in 0..side {
                    let expected_white = col >= x && col < x + w && row >= y && row < y + h;
                    assert_eq!(
                        frame.stdout[(row * side + col) * 3] > 128,
                        expected_white,
                        "{name}: stretched/cropped at {col},{row}"
                    );
                }
            }
        }
    }

    #[test]
    fn incompatible_video_normalises_each_episode_before_concat() {
        let Some(tools) = tools() else {
            return;
        };
        let temp = TempDirectory::create(&std::env::temp_dir()).unwrap();
        let mut inputs = Vec::new();
        for (i, filter) in [
            (1, "testsrc2=size=128x192:rate=12"),
            (2, "testsrc2=size=192x128:rate=24"),
        ] {
            let path = temp.path.join(format!("{i}.mp4"));
            assert!(tools
                .ffmpeg_command()
                .args([
                    "-v", "error", "-f", "lavfi", "-i", filter, "-t", "1", "-an", "-c:v",
                    "libx264", "-pix_fmt", "yuv420p"
                ])
                .arg(&path)
                .status()
                .unwrap()
                .success());
            inputs.push(snapshot(path, i));
        }
        let request = MergeRequest {
            series_root: temp.path.clone(),
            output_file_name: "不同参数.mp4".into(),
            inputs,
            transcode_h264: false,
            square_canvas: false,
            mode: Some(MergeMode::Auto),
            quality: MergeQuality::Balanced,
            conflict_policy: MergeConflictPolicy::FailIfExists,
        };
        let mut stages = Vec::new();
        let result = run_merge_with_budget(
            &tools,
            request,
            &CancellationToken::default(),
            Some(super::super::scheduling::ExecutionBudget {
                cpu_threads: 2,
                force_cpu: true,
                merge: true,
                memory: 0,
                gpu_memory: 0,
            }),
            |p| stages.push(p.stage),
        )
        .unwrap();
        assert!(stages.iter().any(|s| s.contains("CPU")));
        assert!(!stages.iter().any(|s| s.contains("GPU 编码")));
        let probe = probe_media(&tools, &result.output_path).unwrap();
        assert_eq!(
            (probe.video.width, probe.video.height),
            (Some(128), Some(192))
        );
        assert!((probe.duration_seconds - 2.0).abs() < 0.03);
        assert_eq!(frame_hashes(&tools, &result.output_path).len(), 24);
    }

    #[test]
    fn audio_mismatch_and_silent_episode_keep_all_video_frames_lossless() {
        let Some(tools) = tools() else {
            return;
        };
        let temp = TempDirectory::create(&std::env::temp_dir()).unwrap();
        let source = temp.path.join("video.mp4");
        let status = tools
            .ffmpeg_command()
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=128x192:rate=12",
                "-t",
                "1",
                "-an",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());
        let mut inputs = Vec::new();
        for (i, rate, channels) in [(1, "44100", "1"), (2, "48000", "2")] {
            let path = temp.path.join(format!("第 {i} 集.mp4"));
            let out = tools
                .ffmpeg_command()
                .args(["-v", "error", "-i"])
                .arg(&source)
                .args([
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("sine=frequency=440:sample_rate={rate}"),
                    "-map",
                    "0:v",
                    "-map",
                    "1:a",
                    "-c:v",
                    "copy",
                    "-c:a",
                    "aac",
                    "-ac",
                    channels,
                    "-t",
                    "1",
                ])
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            inputs.push(snapshot(path, i));
        }
        inputs.push(snapshot(source.clone(), 3));
        let mut details = inputs
            .iter()
            .map(|i| probe_details(&tools, &i.path).unwrap())
            .collect::<Vec<_>>();
        assert!(can_copy_video(&details));
        details[1].video_config[0] = serde_json::json!("different decoder configuration");
        assert!(!can_copy_video(&details));
        let mut request = MergeRequest {
            series_root: temp.path.clone(),
            output_file_name: "合并.mp4".into(),
            inputs,
            transcode_h264: false,
            square_canvas: false,
            mode: Some(MergeMode::Copy),
            quality: MergeQuality::High,
            conflict_policy: MergeConflictPolicy::FailIfExists,
        };
        let error = run_merge(
            &tools,
            request.clone(),
            &CancellationToken::default(),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(error.code, "MERGE_TRANSCODE_REQUIRED");
        request.mode = Some(MergeMode::Auto);
        let mut stages = Vec::new();
        let result = run_merge(&tools, request, &CancellationToken::default(), |p| {
            stages.push(p.stage)
        })
        .unwrap();
        assert!(stages.iter().any(|s| s.contains("统一音频")));
        assert!(!stages.iter().any(|s| s.contains("GPU")));
        assert_eq!(
            frame_hashes(&tools, &result.output_path),
            (0..3)
                .flat_map(|_| frame_hashes(&tools, &source))
                .collect::<Vec<_>>()
        );
        let probe = probe_media(&tools, &result.output_path).unwrap();
        assert!((probe.duration_seconds - 3.0).abs() < 0.03);
        assert_eq!(probe.audio.unwrap().sample_rate, Some(48000));
        // Identical streams retain the single-pass copy path.
        let input = snapshot(source, 1);
        let request = MergeRequest {
            series_root: temp.path.clone(),
            output_file_name: "无损.mp4".into(),
            inputs: vec![
                input.clone(),
                MergeInput {
                    episode_index: 2,
                    ..input
                },
            ],
            transcode_h264: false,
            square_canvas: false,
            mode: Some(MergeMode::Auto),
            quality: MergeQuality::High,
            conflict_policy: MergeConflictPolicy::FailIfExists,
        };
        let mut stages = Vec::new();
        run_merge(&tools, request, &CancellationToken::default(), |p| {
            stages.push(p.stage)
        })
        .unwrap();
        assert!(stages.iter().any(|s| s == "正在复制视频流"));
        assert!(!stages
            .iter()
            .any(|s| s.contains("统一音频") || s.contains("GPU")));
    }
}
