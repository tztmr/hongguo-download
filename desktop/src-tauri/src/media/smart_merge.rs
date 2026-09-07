//! New merge policy. Legacy persisted requests continue through the original executor.
use super::super::model::{MergeMode, MergeQuality};
use super::*;

const SYNC_TOLERANCE: f64 = 0.12;

struct CheckedProbe {
    media: MediaProbe,
    video_start: f64,
    video_duration: f64,
    audio_timing: Option<(f64, f64)>,
    // Include decoder configuration and display/color metadata, not only codec name.
    configuration: Vec<serde_json::Value>,
}

fn timing_error(message: impl Into<String>) -> AppError {
    AppError::new("MERGE_TIMELINE_INVALID", message)
}

fn number(value: &serde_json::Value, key: &str) -> Result<f64, AppError> {
    value[key]
        .as_str()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .ok_or_else(|| timing_error("无法读取完整音视频时间轴，请检查源视频"))
}

fn checked_probe(
    tools: &MediaTools,
    path: &Path,
    temp: Option<&TempDirectory>,
    control: &CancellationToken,
) -> Result<CheckedProbe, AppError> {
    let mut command = tools.ffprobe_command();
    if let Some(temp) = temp {
        temp.directory_path()?;
        set_child_working_directory(&mut command, temp.directory.as_raw_fd());
    }
    command
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-show_data_hash",
            "sha256",
            "-of",
            "json",
        ])
        .arg(path);
    let output = controlled_read(command, control, |reader| {
        let mut data = Vec::new();
        reader
            .take(4 * 1024 * 1024)
            .read_to_end(&mut data)
            .map_err(merge_io_error)?;
        Ok(data)
    })?;
    let media = parse_probe_json(&output)?;
    let raw: serde_json::Value = serde_json::from_slice(&output).map_err(merge_io_error)?;
    let streams = raw["streams"].as_array().ok_or_else(invalid_probe)?;
    let video = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .ok_or_else(invalid_probe)?;
    let video_start = number(video, "start_time")?;
    let video_duration = number(video, "duration")?;
    if video_duration <= 0.0 {
        return Err(timing_error("源视频时长无效"));
    }
    let audio_timing = streams
        .iter()
        .find(|s| s["codec_type"] == "audio")
        .map(|s| {
            let duration = number(s, "duration")?;
            if duration <= 0.0 {
                return Err(timing_error("源音轨时长无效"));
            }
            Ok((number(s, "start_time")?, duration))
        })
        .transpose()?;
    let configuration = streams
        .iter()
        .filter(|s| s["codec_type"] == "video" || s["codec_type"] == "audio")
        .map(|s| {
            serde_json::Value::Array(
                [
                    "codec_type",
                    "codec_name",
                    "profile",
                    "pix_fmt",
                    "sample_aspect_ratio",
                    "channel_layout",
                    "extradata_hash",
                    "color_space",
                    "color_transfer",
                    "color_primaries",
                    "side_data_list",
                ]
                .iter()
                .map(|key| s[*key].clone())
                .collect(),
            )
        })
        .collect();
    Ok(CheckedProbe {
        media,
        video_start,
        video_duration,
        audio_timing,
        configuration,
    })
}

// Each child, including ffprobe, participates in task pause/resume/cancel. Packet output is
// reduced while reading, so validation memory does not grow with season length.
fn controlled_read<T: Send + 'static>(
    mut command: Command,
    control: &CancellationToken,
    read: impl FnOnce(BufReader<std::process::ChildStdout>) -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    if control.is_cancelled() {
        return Err(cancelled_error());
    }
    control.prepare_command(&mut command);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(merge_io_error)?;
    if let Err(error) = control.register_child(&mut child) {
        terminate_and_wait(&mut child);
        control.clear_child(child.id());
        return Err(error);
    }
    let id = child.id();
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let reader = thread::spawn(move || read(BufReader::new(stdout)));
    let errors = thread::spawn(move || {
        let mut bytes = Vec::new();
        // Drain all diagnostics, keeping only the first 16 KiB.
        for line in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
            if bytes.len() < 16384 {
                bytes.extend(line.into_iter().take(16384 - bytes.len()));
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let status = loop {
        if control.is_cancelled() {
            terminate_and_wait(&mut child);
            control.kill_registered_group();
            break Err(cancelled_error());
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(15)),
            Err(error) => {
                terminate_and_wait(&mut child);
                break Err(merge_io_error(error));
            }
        }
    };
    control.clear_child(id);
    let result = reader
        .join()
        .map_err(|_| timing_error("时间轴检查意外中断"))?;
    let stderr = errors.join().unwrap_or_default();
    if !status?.success() {
        return Err(AppError::with_cause(
            "FFPROBE_FAILED",
            "媒体时间轴检查失败",
            stderr,
        ));
    }
    result
}

fn copy_safe(probes: &[CheckedProbe]) -> bool {
    let Some(first) = probes.first() else {
        return false;
    };
    let mut accumulated_padding = 0.0;
    for probe in probes {
        if probe.media.video != first.media.video
            || probe.media.audio != first.media.audio
            || probe.configuration != first.configuration
        {
            return false;
        }
        if probe.video_start.abs() > 0.002 {
            return false;
        }
        accumulated_padding += (probe.media.duration_seconds - probe.video_duration).abs();
        if let Some((start, duration)) = probe.audio_timing {
            if (start - probe.video_start).abs() > 0.025 {
                return false;
            }
            accumulated_padding += (duration - probe.video_duration).abs();
        }
    }
    accumulated_padding <= SYNC_TOLERANCE
}

fn video_copy_safe(probes: &[CheckedProbe]) -> bool {
    let Some(first) = probes.first() else {
        return false;
    };
    let configuration = |probe: &CheckedProbe| {
        probe
            .configuration
            .iter()
            .find(|value| value[0] == "video")
            .cloned()
    };
    probes.iter().all(|probe| {
        probe.media.video == first.media.video
            && configuration(probe) == configuration(first)
            && probe.video_start.abs() <= 0.002
    })
}

fn frame_rate(signature: &StreamSignature) -> Result<(String, f64), AppError> {
    let text = signature.frame_rate.as_deref().ok_or_else(invalid_probe)?;
    let (a, b) = text.split_once('/').ok_or_else(invalid_probe)?;
    let a: u32 = a.parse().map_err(|_| invalid_probe())?;
    let b: u32 = b.parse().map_err(|_| invalid_probe())?;
    let fps = f64::from(a) / f64::from(b);
    if b == 0 || !(1.0..=120.0).contains(&fps) {
        return Err(timing_error("暂不支持此视频帧率"));
    }
    Ok((format!("{a}/{b}"), fps))
}

fn quality_args(
    quality: MergeQuality,
    encoder: VideoEncoder,
    pixels_per_second: f64,
) -> Vec<String> {
    let (factor, crf) = match quality {
        MergeQuality::High => (0.20, "18"),
        MergeQuality::Balanced => (0.12, "22"),
        MergeQuality::Compact => (0.065, "27"),
    };
    let mut args = vec![
        "-c:v".into(),
        match encoder {
            VideoEncoder::VideoToolbox => "h264_videotoolbox",
            VideoEncoder::LibX264 => "libx264",
        }
        .into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
    ];
    match encoder {
        VideoEncoder::VideoToolbox => args.extend([
            "-b:v".into(),
            format!(
                "{}",
                (pixels_per_second * factor).clamp(500_000.0, 80_000_000.0) as u64
            ),
        ]),
        VideoEncoder::LibX264 => {
            args.extend(["-crf".into(), crf.into(), "-preset".into(), "fast".into()])
        }
    }
    args
}

fn audio_bitrate(quality: MergeQuality) -> &'static str {
    match quality {
        MergeQuality::High => "192k",
        MergeQuality::Balanced => "160k",
        MergeQuality::Compact => "128k",
    }
}

struct NormalizeSpec {
    width: u32,
    height: u32,
    rate: String,
    fps: f64,
    audio: bool,
    copy_video: bool,
    quality: MergeQuality,
}

fn normalize_args(
    input: &Path,
    output: &Path,
    probe: &CheckedProbe,
    spec: &NormalizeSpec,
    duration: f64,
    encoder: VideoEncoder,
) -> Vec<String> {
    let mut args = vec![
        "-hide_banner".into(),
        "-y".into(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
    ];
    if spec.audio && probe.audio_timing.is_none() {
        args.extend([
            "-f".into(),
            "lavfi".into(),
            "-i".into(),
            "anullsrc=r=48000:cl=stereo".into(),
        ]);
    }
    args.extend(["-map".into(), "0:v:0".into()]);
    if spec.audio {
        args.extend([
            "-map".into(),
            if probe.audio_timing.is_some() {
                "0:a:0"
            } else {
                "1:a:0"
            }
            .into(),
        ]);
        // Preserve the source's audio/video offset; fit only the tail to the video timeline.
        let offset = probe
            .audio_timing
            .map(|(start, _)| start - probe.video_start)
            .unwrap_or(0.0);
        args.extend(["-af".into(), format!("asetpts=PTS-STARTPTS+{offset:.9}/TB,aresample=48000:async=1:first_pts=0,apad,atrim=duration={duration:.9}"), "-c:a".into(), "pcm_s16le".into(), "-ar".into(), "48000".into(), "-ac".into(), "2".into()]);
    }
    if spec.copy_video {
        args.extend(["-c:v".into(), "copy".into()]);
    } else {
        args.extend(["-vf".into(), format!("setpts=PTS-STARTPTS,fps={},scale={}:{}:force_original_aspect_ratio=decrease:force_divisible_by=2,pad={}:{}:(ow-iw)/2:(oh-ih)/2,setsar=1", spec.rate, spec.width, spec.height, spec.width, spec.height)]);
        args.extend(quality_args(
            spec.quality,
            encoder,
            f64::from(spec.width) * f64::from(spec.height) * spec.fps,
        ));
    }
    args.extend([
        "-t".into(),
        format!("{duration:.9}"),
        "-video_track_timescale".into(),
        "90000".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        output.to_string_lossy().into_owned(),
    ]);
    args
}

pub(super) fn run(
    tools: &MediaTools,
    request: &MergeRequest,
    mode: MergeMode,
    control: &CancellationToken,
    progress: &mut dyn FnMut(MergeProgress),
) -> Result<MergeResult, AppError> {
    run_with_encoder(
        tools,
        request,
        mode,
        control,
        progress,
        VideoEncoder::VideoToolbox,
    )
}

fn run_with_encoder(
    tools: &MediaTools,
    request: &MergeRequest,
    mode: MergeMode,
    control: &CancellationToken,
    progress: &mut dyn FnMut(MergeProgress),
    encoder: VideoEncoder,
) -> Result<MergeResult, AppError> {
    let destination = ValidatedDestination::open(&request.series_root, &request.output_file_name)?;
    let mut inputs = prepare_input_identities(&request.inputs)?;
    inputs.sort_by_key(|input| input.episode_index);
    let normalized_inputs: Vec<_> = inputs.iter().map(|input| input.input.clone()).collect();
    let source_lines = concat_lines(&normalized_inputs)?;
    let mut probes = Vec::with_capacity(inputs.len());
    for input in &inputs {
        progress(MergeProgress {
            stage: "probing".into(),
            percent: 0.0,
            terminal: false,
        });
        probes.push(checked_probe(tools, &input.path, None, control)?);
    }
    revalidate_input_identities(&inputs)?;
    let safe = copy_safe(&probes);
    if mode == MergeMode::Copy && !safe {
        return Err(AppError::new(
            "MERGE_TRANSCODE_REQUIRED",
            "各集参数或时间轴不适合无损拼接，请选择智能合并或 H.264 转码",
        ));
    }
    let audio = probes.iter().any(|p| p.audio_timing.is_some());
    // MP4 cannot reliably preserve per-episode AAC priming/padding when copying.
    // Keep compatible video lossless, but decode each episode's audio separately.
    let copy = mode != MergeMode::Transcode
        && safe
        && (mode == MergeMode::Copy || !audio || probes.len() == 1);
    let copy_video = mode == MergeMode::Auto && video_copy_safe(&probes);
    let mut expected_duration: f64 = probes.iter().map(|p| p.video_duration).sum();
    destination.verify_public_root_entry()?;
    let mut temp = TempDirectory::create(&destination)?;
    let mut tracker = ProgressTracker::new(expected_duration);
    if copy {
        temp.write_concat(source_lines)?;
        emit_stage(&mut tracker, "stream-copy", progress);
        let outcome = run_ffmpeg(
            tools,
            &temp,
            stream_copy_args(&temp.concat_path()?, &temp.output_path()?),
            control,
            &mut tracker,
            progress,
        )?;
        if !outcome.success {
            return Err(ffmpeg_error(outcome.stderr));
        }
    } else {
        let first = &probes[0].media.video;
        let width = first
            .width
            .filter(|v| *v >= 2 && *v <= 8192)
            .ok_or_else(invalid_probe)?
            & !1;
        let height = first
            .height
            .filter(|v| *v >= 2 && *v <= 8192)
            .ok_or_else(invalid_probe)?
            & !1;
        let (rate, fps) = frame_rate(first)?;
        let spec = NormalizeSpec {
            width,
            height,
            rate,
            fps,
            audio,
            copy_video,
            quality: request.quality,
        };
        let durations: Vec<_> = probes
            .iter()
            .map(|p| {
                if copy_video {
                    p.video_duration
                } else {
                    (p.video_duration * fps).round().max(1.0) / fps
                }
            })
            .collect();
        expected_duration = durations.iter().sum();
        let mut completed = 0.0;
        let mut lines = Vec::new();
        for (index, ((input, probe), duration)) in
            inputs.iter().zip(&probes).zip(&durations).enumerate()
        {
            revalidate_input_identities(&inputs)?;
            let name = format!("segment-{index:06}.mov");
            temp.extra_files.push(name.clone());
            let path = Path::new(&name);
            let mut segment_tracker = ProgressTracker::new(*duration);
            let mut segment_progress = |mut event: MergeProgress| {
                event.percent =
                    ((completed + duration * event.percent / 100.0) / expected_duration * 85.0)
                        .min(85.0);
                progress(event);
            };
            emit_stage(
                &mut segment_tracker,
                if copy_video {
                    "audio-normalizing"
                } else if matches!(encoder, VideoEncoder::VideoToolbox) {
                    "hardware-transcode"
                } else {
                    "software-fallback"
                },
                &mut segment_progress,
            );
            let outcome = run_ffmpeg(
                tools,
                &temp,
                normalize_args(&input.path, path, probe, &spec, *duration, encoder),
                control,
                &mut segment_tracker,
                &mut segment_progress,
            )?;
            if !outcome.success {
                if !matches!(encoder, VideoEncoder::VideoToolbox)
                    || !is_videotoolbox_failure(&outcome.stderr)
                {
                    return Err(ffmpeg_error(outcome.stderr));
                }
                // Restart all segments with one encoder: mixing hardware/software codec
                // configurations in MP4 could make later segments undecodable.
                drop(temp);
                return run_with_encoder(
                    tools,
                    request,
                    MergeMode::Transcode,
                    control,
                    progress,
                    VideoEncoder::LibX264,
                );
            }
            lines.push(format!("file '{name}'"));
            lines.push(format!("duration {duration:.9}"));
            completed += duration;
        }
        temp.write_concat(lines)?;
        let mut args = common_args(&temp.concat_path()?);
        args.extend([
            "-c:v".into(),
            "copy".into(),
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            audio_bitrate(request.quality).into(),
        ]);
        let args = finish_args(args, &temp.output_path()?);
        tracker = ProgressTracker::new(expected_duration);
        let mut final_progress = |mut event: MergeProgress| {
            event.percent = 85.0 + event.percent * 0.1;
            progress(event);
        };
        emit_stage(&mut tracker, "merging", &mut final_progress);
        let outcome = run_ffmpeg(
            tools,
            &temp,
            args,
            control,
            &mut tracker,
            &mut final_progress,
        )?;
        if !outcome.success {
            return Err(ffmpeg_error(outcome.stderr));
        }
    }
    progress(MergeProgress {
        stage: "validating".into(),
        percent: 99.0,
        terminal: false,
    });
    let output = checked_probe(tools, &temp.output_path()?, Some(&temp), control)?;
    let validation = (|| {
        validate_merged_output(&output.media, audio, expected_duration)?;
        if (output.video_duration - expected_duration).abs() > SYNC_TOLERANCE {
            return Err(timing_error("合并视频时长与各集总时长不一致"));
        }
        validate_packets(tools, &temp, &output, control)?;
        validate_audio_samples(tools, &temp, &output, control)
    })();
    if let Err(error) = validation {
        if copy
            && mode == MergeMode::Auto
            && matches!(
                error.code.as_str(),
                "MERGE_TIMELINE_INVALID" | "MERGE_VALIDATION_FAILED"
            )
        {
            drop(temp);
            return run_with_encoder(
                tools,
                request,
                MergeMode::Transcode,
                control,
                progress,
                encoder,
            );
        }
        return Err(error);
    }
    revalidate_input_identities(&inputs)?;
    if control.is_cancelled() {
        return Err(cancelled_error());
    }
    let output_path = publish_output(
        &destination,
        &temp,
        &request.output_file_name,
        request.conflict_policy,
    )?;
    Ok(MergeResult { output_path })
}

#[derive(Default, Debug)]
struct PacketTimeline {
    pending: Vec<(f64, f64)>,
    start: Option<f64>,
    end: f64,
    max_gap: f64,
    max_overlap: f64,
    max_packet_duration: f64,
    packets: usize,
}
impl PacketTimeline {
    fn push(&mut self, start: f64, duration: f64) {
        self.pending.push((start, duration));
        self.packets += 1;
        self.max_packet_duration = self.max_packet_duration.max(duration);
        // H.264/HEVC B-frame reordering is bounded; keep a generous reorder window.
        if self.pending.len() > 64 {
            self.consume_first();
        }
    }
    fn consume_first(&mut self) {
        self.pending.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (start, duration) = self.pending.remove(0);
        if self.start.is_some() {
            self.max_gap = self.max_gap.max(start - self.end);
            self.max_overlap = self.max_overlap.max(self.end - start);
        } else {
            self.start = Some(start);
            self.end = start;
        }
        self.end = self.end.max(start + duration);
    }
    fn finish(&mut self) {
        while !self.pending.is_empty() {
            self.consume_first();
        }
    }
    fn read(reader: impl BufRead) -> Result<Self, AppError> {
        let mut timeline = Self::default();
        for line in reader.lines() {
            let line = line.map_err(merge_io_error)?;
            if line.trim().is_empty() {
                continue;
            }
            let mut pts = None;
            let mut duration = None;
            for field in line.split('|') {
                if let Some((key, value)) = field.split_once('=') {
                    match key {
                        "pts_time" => pts = value.parse::<f64>().ok(),
                        "duration_time" => duration = value.parse::<f64>().ok(),
                        _ => {}
                    }
                }
            }
            if let (Some(start), Some(duration)) = (pts, duration) {
                if !start.is_finite() || !duration.is_finite() || duration <= 0.0 {
                    return Err(timing_error("成片存在无效媒体时间戳"));
                }
                timeline.push(start, duration);
            } else if line.contains("pts_time=") {
                return Err(timing_error("成片存在缺失媒体时间戳"));
            }
        }
        timeline.finish();
        if timeline.packets == 0 {
            return Err(timing_error("成片缺少可检查的媒体数据"));
        }
        Ok(timeline)
    }
}

fn validate_packets(
    tools: &MediaTools,
    temp: &TempDirectory,
    probe: &CheckedProbe,
    control: &CancellationToken,
) -> Result<(), AppError> {
    let video = packet_track(tools, temp, "v:0", control)?;
    if video.max_gap > SYNC_TOLERANCE
        || video.max_overlap > SYNC_TOLERANCE
        || video.start.unwrap_or(0.0).abs() > SYNC_TOLERANCE
        || (video.end - probe.video_duration).abs() > SYNC_TOLERANCE
    {
        return Err(timing_error("成片视频时间轴存在缺口或时长异常"));
    }
    if probe.audio_timing.is_some() {
        let audio = packet_track(tools, temp, "a:0", control)?;
        if audio.max_gap > SYNC_TOLERANCE
            || audio.max_overlap > SYNC_TOLERANCE
            || audio.max_packet_duration > SYNC_TOLERANCE + 0.001
            || (audio.start.unwrap_or(0.0) - video.start.unwrap_or(0.0)).abs() > SYNC_TOLERANCE
            || (audio.end - video.end).abs() > SYNC_TOLERANCE
        {
            return Err(timing_error(
                "成片音视频时间轴不一致，已阻止发布；请检查源视频或使用 H.264 转码",
            ));
        }
    }
    Ok(())
}

fn packet_track(
    tools: &MediaTools,
    temp: &TempDirectory,
    selector: &str,
    control: &CancellationToken,
) -> Result<PacketTimeline, AppError> {
    temp.directory_path()?;
    let mut command = tools.ffprobe_command();
    set_child_working_directory(&mut command, temp.directory.as_raw_fd());
    command
        .args([
            "-v",
            "error",
            "-select_streams",
            selector,
            "-show_packets",
            "-show_entries",
            "packet=pts_time,duration_time",
            "-of",
            "compact=p=0:nk=0",
        ])
        .arg(temp.output_path()?);
    controlled_read(command, control, PacketTimeline::read)
}

fn validate_audio_samples(
    tools: &MediaTools,
    temp: &TempDirectory,
    probe: &CheckedProbe,
    control: &CancellationToken,
) -> Result<(), AppError> {
    let Some(audio) = &probe.media.audio else {
        return Ok(());
    };
    let rate = audio
        .sample_rate
        .filter(|rate| *rate > 0)
        .ok_or_else(invalid_probe)?;
    temp.directory_path()?;
    let mut command = tools.ffprobe_command();
    set_child_working_directory(&mut command, temp.directory.as_raw_fd());
    command
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_frames",
            "-show_entries",
            "frame=nb_samples",
            "-of",
            "compact=p=0:nk=0",
        ])
        .arg(temp.output_path()?);
    let samples = controlled_read(command, control, |reader| {
        let mut samples = 0u64;
        for line in reader.lines() {
            for field in line.map_err(merge_io_error)?.split('|') {
                if let Some(value) = field.strip_prefix("nb_samples=") {
                    let count = value
                        .parse::<u64>()
                        .map_err(|_| timing_error("音频解码样本无效"))?;
                    samples = samples
                        .checked_add(count)
                        .ok_or_else(|| timing_error("音频解码样本溢出"))?;
                }
            }
        }
        Ok(samples)
    })?;
    if samples == 0
        || (samples as f64 / f64::from(rate) - probe.video_duration).abs() > SYNC_TOLERANCE
    {
        return Err(timing_error(
            "实际解码音频时长与画面不一致，请使用智能合并校正音轨",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_validation_detects_middle_gap_even_with_matching_total_duration() {
        let track = PacketTimeline::read(std::io::Cursor::new(
            "pts_time=0|duration_time=1\npts_time=3|duration_time=1\n",
        ))
        .unwrap();
        assert_eq!(track.end, 4.0);
        assert_eq!(track.max_gap, 2.0);
    }

    #[test]
    fn reordered_video_packets_are_not_mistaken_for_gaps() {
        let track = PacketTimeline::read(std::io::Cursor::new("pts_time=0|duration_time=0.04\npts_time=0.08|duration_time=0.04\npts_time=0.04|duration_time=0.04\n")).unwrap();
        assert!(track.max_gap < 0.001);
        assert!((track.end - 0.12).abs() < 0.001);
        assert!(
            PacketTimeline::read(std::io::Cursor::new("pts_time=N/A|duration_time=0.04\n"))
                .is_err()
        );
    }

    #[test]
    fn packet_scanning_keeps_bounded_memory_for_long_seasons() {
        let mut track = PacketTimeline::default();
        for i in 0..200_000 {
            track.push(f64::from(i) * 0.04, 0.04);
            assert!(track.pending.len() <= 64);
        }
        track.finish();
        assert!(track.max_gap < 0.001);
        assert!((track.end - 8000.0).abs() < 0.001);
    }

    #[test]
    fn individually_small_audio_padding_cannot_accumulate_across_a_season() {
        let make = || CheckedProbe {
            media: MediaProbe {
                video: StreamSignature {
                    codec_name: "h264".into(),
                    width: Some(160),
                    height: Some(96),
                    frame_rate: Some("30/1".into()),
                    time_base: Some("1/15360".into()),
                    sample_rate: None,
                    channels: None,
                },
                audio: None,
                duration_seconds: 60.02,
            },
            video_start: 0.0,
            video_duration: 60.0,
            audio_timing: Some((0.0, 60.02)),
            configuration: Vec::new(),
        };
        assert!(copy_safe(&[make()]));
        assert!(!copy_safe(&(0..100).map(|_| make()).collect::<Vec<_>>()));
    }

    #[test]
    fn legacy_requests_keep_their_mode_and_new_options_survive_serialization() {
        let base = serde_json::json!({"seriesRoot":"/tmp", "outputFileName":"out.mp4", "inputs":[], "transcodeH264":true});
        let old: MergeRequest = serde_json::from_value(base.clone()).unwrap();
        assert_eq!(old.mode, None);
        assert!(old.transcode_h264);
        let mut new = base;
        new["mode"] = "auto".into();
        new["quality"] = "compact".into();
        let new: MergeRequest = serde_json::from_value(new).unwrap();
        let restored: MergeRequest =
            serde_json::from_slice(&serde_json::to_vec(&new).unwrap()).unwrap();
        assert_eq!(restored.mode, Some(MergeMode::Auto));
        assert_eq!(restored.quality, MergeQuality::Compact);
    }

    #[test]
    fn cancelling_probe_terminates_the_process_and_unblocks_reader() {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "trap 'sleep 3 &' TERM; printf 'ready\\n'; while true; do sleep 3; done",
        ]);
        let control = CancellationToken::new();
        let worker_control = control.clone();
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            controlled_read(command, &worker_control, move |mut reader| {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                sender.send(()).unwrap();
                let mut rest = String::new();
                reader.read_to_string(&mut rest).map_err(merge_io_error)
            })
        });
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        let start = Instant::now();
        control.cancel();
        assert_eq!(worker.join().unwrap().unwrap_err().code, "MERGE_CANCELLED");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hongguo-smart-merge-{}-{:032x}",
                std::process::id(),
                random_u128()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn input(&self, index: u32) -> MergeInput {
            let path = self.0.join(format!("{index}.mp4"));
            let meta = fs::metadata(&path).unwrap();
            MergeInput {
                episode_index: index,
                path,
                size: meta.len(),
                modified_unix_nanos: meta
                    .modified()
                    .unwrap()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    #[ignore = "requires explicitly supplied HONGGUO_TEST_FFMPEG/HONGGUO_TEST_FFPROBE"]
    fn real_tools_smart_merge_copy_mixed_specs_and_audio_padding() {
        let ffmpeg = PathBuf::from(std::env::var_os("HONGGUO_TEST_FFMPEG").expect("set ffmpeg"));
        let tools = MediaTools::from_test_paths(
            ffmpeg.clone(),
            PathBuf::from(std::env::var_os("HONGGUO_TEST_FFPROBE").expect("set ffprobe")),
        );
        for scenario in [
            "copy",
            "mixed",
            "short-audio",
            "transcode",
            "silent",
            "long",
            "gap",
            "fallback",
            "long-transcode",
            "unsafe-copy",
        ] {
            let fixture = Fixture::new();
            let long = scenario.starts_with("long");
            for index in 1..=2 {
                let mut command = Command::new(&ffmpeg);
                let video = if scenario == "long-transcode" {
                    "color=c=blue:s=160x96:d=31:r=1"
                } else if long {
                    "color=c=blue:s=160x96:d=31:r=30"
                } else if scenario == "mixed" && index == 2 {
                    "color=c=green:s=96x160:d=1:r=24"
                } else {
                    "color=c=blue:s=160x96:d=1:r=30"
                };
                command.args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    video,
                ]);
                if scenario != "silent" && !(scenario == "mixed" && index == 2) {
                    command.args([
                        "-f",
                        "lavfi",
                        "-i",
                        if long {
                            "sine=frequency=440:duration=31:sample_rate=48000"
                        } else if scenario == "short-audio" {
                            "sine=frequency=440:duration=0.75:sample_rate=48000"
                        } else {
                            "sine=frequency=440:duration=1:sample_rate=48000"
                        },
                    ]);
                }
                if scenario == "gap" {
                    command.args(["-af", "aselect=not(between(t\\,0.3\\,0.6))"]);
                }
                command
                    .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
                    .arg(fixture.0.join(format!("{index}.mp4")));
                assert!(command.status().unwrap().success());
            }
            let count = if long || scenario == "unsafe-copy" {
                60
            } else {
                2
            };
            for index in 3..=count {
                fs::hard_link(
                    fixture.0.join("1.mp4"),
                    fixture.0.join(format!("{index}.mp4")),
                )
                .unwrap();
            }
            let request = MergeRequest {
                series_root: fixture.0.clone(),
                output_file_name: "全集.mp4".into(),
                inputs: (1..=count).rev().map(|i| fixture.input(i)).collect(),
                transcode_h264: false,
                mode: Some(
                    if scenario == "transcode"
                        || scenario == "fallback"
                        || scenario == "long-transcode"
                    {
                        MergeMode::Transcode
                    } else if scenario == "unsafe-copy" {
                        MergeMode::Copy
                    } else {
                        MergeMode::Auto
                    },
                ),
                quality: if scenario == "transcode" {
                    MergeQuality::Compact
                } else if scenario == "mixed" {
                    MergeQuality::Balanced
                } else {
                    MergeQuality::High
                },
                conflict_policy: MergeConflictPolicy::FailIfExists,
            };
            let fallback_tools = if scenario == "fallback" {
                use std::os::unix::fs::PermissionsExt;
                let wrapper = fixture.0.join("ffmpeg-fallback");
                let executable = ffmpeg.to_string_lossy().replace('\'', "'\"'\"'");
                fs::write(&wrapper, format!("#!/bin/sh\nencoder=0\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; if [ \"$arg\" = h264_videotoolbox ]; then encoder=1; fi; done\nif [ \"$encoder\" = 1 ] && [ \"$last\" = segment-000001.mov ]; then printf '[h264_videotoolbox @ 0x123] Failed to create VTCompressionSession\\n' >&2; exit 1; fi\nexec '{executable}' \"$@\"\n")).unwrap();
                fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
                Some(MediaTools::from_test_paths(
                    wrapper,
                    PathBuf::from(std::env::var_os("HONGGUO_TEST_FFPROBE").unwrap()),
                ))
            } else {
                None
            };
            let selected_tools = fallback_tools.as_ref().unwrap_or(&tools);
            let mut stages = Vec::new();
            let outcome = run_merge(selected_tools, request, &CancellationToken::new(), |e| {
                stages.push(e.stage)
            });
            if scenario == "unsafe-copy" {
                assert_eq!(outcome.unwrap_err().code, "MERGE_TIMELINE_INVALID");
                assert!(!fixture.0.join("合并视频").join("全集.mp4").exists());
                assert!(!fs::read_dir(&fixture.0).unwrap().any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".hongguo-merge-")));
                continue;
            }
            let result = outcome.unwrap_or_else(|e| panic!("{scenario}: {e:?}"));
            if scenario == "fallback" {
                assert!(stages.iter().any(|s| s == "software-fallback"));
                let decoded = Command::new(&ffmpeg)
                    .args(["-v", "error", "-i"])
                    .arg(&result.output_path)
                    .args(["-f", "null", "-"])
                    .output()
                    .unwrap();
                assert!(
                    decoded.status.success() && decoded.stderr.is_empty(),
                    "fallback decode: {}",
                    String::from_utf8_lossy(&decoded.stderr)
                );
            }
            let output = probe_media(&tools, &result.output_path).unwrap();
            let expected = if long { 1860.0 } else { 2.0 };
            assert!(
                (output.duration_seconds - expected).abs() < 0.12,
                "{scenario}: {output:?}"
            );
            if scenario == "silent" {
                assert!(
                    stages.iter().any(|s| s == "stream-copy"),
                    "{scenario}: {stages:?}"
                );
            } else if matches!(scenario, "copy" | "long" | "short-audio" | "gap") {
                assert!(stages.iter().any(|s| s == "audio-normalizing"));
                assert!(!stages.iter().any(|s| s == "hardware-transcode"));
            } else {
                assert!(stages.iter().any(|s| s == "hardware-transcode"));
            }
            assert_eq!(
                fs::read_dir(&fixture.0).unwrap().count(),
                count as usize + if scenario == "fallback" { 2 } else { 1 },
                "private temp files leaked"
            );
        }
    }
}
