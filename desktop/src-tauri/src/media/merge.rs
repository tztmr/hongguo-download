use super::{
    model::{MergeConflictPolicy, MergeInput, MergeRequest},
    tools::MediaTools,
};
use crate::AppError;
use serde::Deserialize;
use std::{
    ffi::{CStr, CString, OsString},
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::MetadataExt,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::{OsStrExt, OsStringExt},
        unix::process::CommandExt,
    },
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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

fn parse_probe_json(bytes: &[u8]) -> Result<MediaProbe, AppError> {
    let raw: RawProbe = serde_json::from_slice(bytes).map_err(|error| {
        AppError::with_cause("FFPROBE_INVALID", "媒体检测结果无效", error.to_string())
    })?;
    let mut videos = raw
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("video"));
    let video = videos.next().ok_or_else(invalid_probe)?;
    if videos.next().is_some() {
        return Err(invalid_probe());
    }
    let audios = raw
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("audio"))
        .collect::<Vec<_>>();
    if audios.len() > 1 {
        return Err(invalid_probe());
    }
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
        video: video_signature(video)?,
        audio: audios
            .first()
            .map(|stream| audio_signature(stream))
            .transpose()?,
        duration_seconds,
    })
}

fn video_signature(stream: &RawStream) -> Result<StreamSignature, AppError> {
    Ok(StreamSignature {
        codec_name: stream.codec_name.clone().ok_or_else(invalid_probe)?,
        width: stream.width,
        height: stream.height,
        frame_rate: stream.r_frame_rate.clone(),
        time_base: stream.time_base.clone(),
        sample_rate: None,
        channels: None,
    })
}

fn audio_signature(stream: &RawStream) -> Result<StreamSignature, AppError> {
    Ok(StreamSignature {
        codec_name: stream.codec_name.clone().ok_or_else(invalid_probe)?,
        width: None,
        height: None,
        frame_rate: None,
        time_base: stream.time_base.clone(),
        sample_rate: stream
            .sample_rate
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(|_| invalid_probe())?,
        channels: stream.channels,
    })
}

fn invalid_probe() -> AppError {
    AppError::new("FFPROBE_INVALID", "媒体检测结果无效")
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

fn concat_lines(inputs: &[MergeInput]) -> Result<Vec<String>, AppError> {
    let mut inputs = inputs.iter().collect::<Vec<_>>();
    inputs.sort_by_key(|input| input.episode_index);
    let mut previous = None;
    inputs
        .into_iter()
        .map(|input| {
            if input.episode_index == 0 {
                return Err(AppError::new("MERGE_INPUT_INVALID", "合并集数必须大于零"));
            }
            if previous == Some(input.episode_index) {
                return Err(AppError::new(
                    "MERGE_INPUT_DUPLICATE_INDEX",
                    "合并集数不得重复",
                ));
            }
            previous = Some(input.episode_index);
            let path = input
                .path
                .to_str()
                .ok_or_else(|| AppError::new("MERGE_INPUT_UNSAFE_PATH", "合并输入路径不安全"))?;
            if path.chars().any(|ch| ch == '\'' || ch.is_control()) {
                return Err(AppError::new(
                    "MERGE_INPUT_UNSAFE_PATH",
                    "合并输入路径不安全",
                ));
            }
            Ok(format!("file '{path}'"))
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VideoEncoder {
    VideoToolbox,
    LibX264,
}

fn common_args(concat: &Path) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-y".into(),
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
        "-i".into(),
        concat.to_string_lossy().into_owned(),
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "0:a:0?".into(),
    ]
}

fn finish_args(mut args: Vec<String>, output: &Path) -> Vec<String> {
    args.extend([
        "-movflags".into(),
        "+faststart".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        output.to_string_lossy().into_owned(),
    ]);
    args
}

fn stream_copy_args(concat: &Path, output: &Path) -> Vec<String> {
    let mut args = common_args(concat);
    args.extend(["-c".into(), "copy".into()]);
    finish_args(args, output)
}

fn transcode_args(concat: &Path, output: &Path, encoder: VideoEncoder) -> Vec<String> {
    let mut args = common_args(concat);
    args.extend([
        "-c:v".into(),
        match encoder {
            VideoEncoder::VideoToolbox => "h264_videotoolbox".into(),
            VideoEncoder::LibX264 => "libx264".into(),
        },
        "-c:a".into(),
        "aac".into(),
    ]);
    finish_args(args, output)
}

#[cfg(test)]
fn validated_output_path(series_root: &Path, output_name: &str) -> Result<PathBuf, AppError> {
    let canonical_root = std::fs::canonicalize(series_root).map_err(|error| {
        AppError::with_cause(
            "MERGE_OUTPUT_INVALID",
            "合并输出目录无效",
            error.to_string(),
        )
    })?;
    validate_output_name(output_name)?;
    let output_dir = canonical_root.join("合并视频");
    if output_dir.exists() {
        let canonical_output_dir = std::fs::canonicalize(&output_dir).map_err(|error| {
            AppError::with_cause(
                "MERGE_OUTPUT_INVALID",
                "合并输出目录无效",
                error.to_string(),
            )
        })?;
        if !canonical_output_dir.starts_with(&canonical_root) {
            return Err(AppError::new(
                "MERGE_OUTPUT_INVALID",
                "合并输出目录越出剧目目录",
            ));
        }
        return Ok(canonical_output_dir.join(output_name));
    }
    Ok(output_dir.join(output_name))
}

fn validate_output_name(output_name: &str) -> Result<(), AppError> {
    let name_path = Path::new(output_name);
    let mut components = name_path.components();
    let safe_component = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && name_path.extension().and_then(|value| value.to_str()) == Some("mp4")
        && !output_name.chars().any(char::is_control);
    if !safe_component {
        return Err(AppError::new("MERGE_OUTPUT_INVALID", "合并输出文件名无效"));
    }
    Ok(())
}

struct ValidatedDestination {
    parent: OwnedFd,
    root: OwnedFd,
    canonical_root: PathBuf,
    root_entry: CString,
    device: u64,
    inode: u64,
}

impl ValidatedDestination {
    fn open(series_root: &Path, output_name: &str) -> Result<Self, AppError> {
        validate_output_name(output_name)?;
        let canonical_root = fs::canonicalize(series_root).map_err(output_changed_error)?;
        let parent_path = canonical_root
            .parent()
            .ok_or_else(|| output_changed_error("series root has no parent"))?;
        let root_name = canonical_root
            .file_name()
            .ok_or_else(|| output_changed_error("series root has no directory entry"))?;
        let root_entry = CString::new(root_name.as_bytes())
            .map_err(|_| output_changed_error("series root name contains NUL"))?;
        let parent = open_directory_nofollow(parent_path)?;
        // SAFETY: parent and root_entry are valid; the returned descriptor is checked.
        let raw_root = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                root_entry.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if raw_root < 0 {
            return Err(output_changed_error(std::io::Error::last_os_error()));
        }
        // SAFETY: raw_root is a newly opened descriptor owned by this function.
        let root = unsafe { OwnedFd::from_raw_fd(raw_root) };
        let (device, inode) = directory_identity(root.as_raw_fd())?;
        let destination = Self {
            parent,
            root,
            canonical_root,
            root_entry,
            device,
            inode,
        };
        destination.verify_public_root_entry()?;
        destination.validate_existing_output_entry()?;
        Ok(destination)
    }

    fn verify_public_root_entry(&self) -> Result<(), AppError> {
        verify_directory_entry(
            self.parent.as_raw_fd(),
            &self.root_entry,
            self.device,
            self.inode,
            "series root identity changed",
        )
    }

    fn validate_existing_output_entry(&self) -> Result<(), AppError> {
        let output_entry = output_directory_entry();
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: root and output_entry are valid and stat is writable.
        let result = unsafe {
            libc::fstatat(
                self.root.as_raw_fd(),
                output_entry.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(());
            }
            return Err(output_changed_error(error));
        }
        // SAFETY: fstatat succeeded and initialized stat.
        let stat = unsafe { stat.assume_init() };
        if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
            return Err(output_changed_error("output entry is not a directory"));
        }
        Ok(())
    }

    fn output_path(&self, output_name: &str) -> PathBuf {
        self.canonical_root.join("合并视频").join(output_name)
    }
}

struct OutputDirectory {
    output: OwnedFd,
    device: u64,
    inode: u64,
}

impl OutputDirectory {
    fn open_or_create(destination: &ValidatedDestination) -> Result<Self, AppError> {
        let output_name = output_directory_entry();
        // SAFETY: root is a live directory descriptor and output_name is valid.
        let mkdir_result =
            unsafe { libc::mkdirat(destination.root.as_raw_fd(), output_name.as_ptr(), 0o755) };
        if mkdir_result != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(output_changed_error(error));
            }
        }
        // SAFETY: arguments are valid; the returned descriptor is checked before ownership.
        let raw_output = unsafe {
            libc::openat(
                destination.root.as_raw_fd(),
                output_name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if raw_output < 0 {
            return Err(output_changed_error(std::io::Error::last_os_error()));
        }
        // SAFETY: raw_output is a newly opened descriptor owned by this function.
        let output = unsafe { OwnedFd::from_raw_fd(raw_output) };
        let (device, inode) = directory_identity(output.as_raw_fd())?;
        let directory = Self {
            output,
            device,
            inode,
        };
        directory.verify_entry(destination)?;
        Ok(directory)
    }

    fn verify_entry(&self, destination: &ValidatedDestination) -> Result<(), AppError> {
        verify_directory_entry(
            destination.root.as_raw_fd(),
            &output_directory_entry(),
            self.device,
            self.inode,
            "output directory identity changed",
        )
    }
}

fn output_directory_entry() -> CString {
    CString::new("合并视频").expect("fixed directory has no NUL")
}

fn verify_directory_entry(
    parent_fd: libc::c_int,
    entry: &CString,
    expected_device: u64,
    expected_inode: u64,
    mismatch: &str,
) -> Result<(), AppError> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: parent_fd and entry are valid and stat points to writable storage.
    let result = unsafe {
        libc::fstatat(
            parent_fd,
            entry.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(output_changed_error(std::io::Error::last_os_error()));
    }
    // SAFETY: fstatat succeeded and initialized stat.
    let stat = unsafe { stat.assume_init() };
    let is_directory = stat.st_mode & libc::S_IFMT == libc::S_IFDIR;
    if !is_directory || stat.st_dev as u64 != expected_device || stat.st_ino != expected_inode {
        return Err(output_changed_error(mismatch));
    }
    Ok(())
}

fn open_directory_nofollow(path: &Path) -> Result<OwnedFd, AppError> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| output_changed_error("directory path contains NUL"))?;
    // SAFETY: path is a valid C string; the returned descriptor is checked before ownership.
    let raw = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if raw < 0 {
        return Err(output_changed_error(std::io::Error::last_os_error()));
    }
    // SAFETY: raw is a newly opened descriptor owned by this function.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

fn directory_identity(fd: libc::c_int) -> Result<(u64, u64), AppError> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fd is live and stat points to writable storage.
    let result = unsafe { libc::fstat(fd, stat.as_mut_ptr()) };
    if result != 0 {
        return Err(output_changed_error(std::io::Error::last_os_error()));
    }
    // SAFETY: fstat returned success and initialized stat.
    let stat = unsafe { stat.assume_init() };
    Ok((stat.st_dev as u64, stat.st_ino))
}

fn publish_output(
    destination: &ValidatedDestination,
    temp_directory: &TempDirectory,
    output_name: &str,
    conflict_policy: MergeConflictPolicy,
) -> Result<PathBuf, AppError> {
    temp_directory.verify_identity()?;
    publish_output_impl(
        destination,
        temp_directory.directory.as_raw_fd(),
        &temporary_output_entry(),
        output_name,
        conflict_policy,
        || {},
    )
}

#[cfg(test)]
fn publish_output_with_hook<F>(
    series_root: &Path,
    temp_output: &Path,
    output_name: &str,
    conflict_policy: MergeConflictPolicy,
    hook: F,
) -> Result<PathBuf, AppError>
where
    F: FnOnce(),
{
    let destination = ValidatedDestination::open(series_root, output_name)?;
    let source_parent = temp_output
        .parent()
        .ok_or_else(|| output_changed_error("temporary output has no parent"))?;
    let source_name = temp_output
        .file_name()
        .ok_or_else(|| output_changed_error("temporary output has no filename"))?;
    let source_directory = open_directory_nofollow(source_parent)?;
    let source_entry = CString::new(source_name.as_bytes())
        .map_err(|_| output_changed_error("temporary output filename contains NUL"))?;
    publish_output_impl(
        &destination,
        source_directory.as_raw_fd(),
        &source_entry,
        output_name,
        conflict_policy,
        hook,
    )
}

fn publish_output_impl<F>(
    destination: &ValidatedDestination,
    source_fd: libc::c_int,
    source_entry: &CString,
    output_name: &str,
    conflict_policy: MergeConflictPolicy,
    hook: F,
) -> Result<PathBuf, AppError>
where
    F: FnOnce(),
{
    destination.verify_public_root_entry()?;
    let directory = OutputDirectory::open_or_create(destination)?;
    let output_name_c = CString::new(output_name.as_bytes())
        .map_err(|_| AppError::new("MERGE_OUTPUT_INVALID", "合并输出文件名无效"))?;

    hook();
    destination.verify_public_root_entry()?;
    directory.verify_entry(destination)?;
    let result = match conflict_policy {
        MergeConflictPolicy::FailIfExists => rename_no_replace(
            source_fd,
            source_entry,
            directory.output.as_raw_fd(),
            &output_name_c,
        ),
        MergeConflictPolicy::Overwrite => {
            // SAFETY: both C strings and the destination directory descriptor are valid.
            unsafe {
                libc::renameat(
                    source_fd,
                    source_entry.as_ptr(),
                    directory.output.as_raw_fd(),
                    output_name_c.as_ptr(),
                )
            }
        }
    };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(AppError::new(
                "MERGE_OUTPUT_EXISTS",
                "合并输出已存在，默认不覆盖",
            ));
        }
        return Err(AppError::with_cause(
            "MERGE_PUBLISH_FAILED",
            "合并输出发布失败",
            error.to_string(),
        ));
    }

    let final_identity = destination
        .verify_public_root_entry()
        .and_then(|()| directory.verify_entry(destination));
    if let Err(error) = final_identity {
        // SAFETY: the output descriptor remains pinned even if its directory entry changed.
        let _ = unsafe { libc::unlinkat(directory.output.as_raw_fd(), output_name_c.as_ptr(), 0) };
        return Err(error);
    }
    // SAFETY: output is a live directory descriptor; fsync has no pointer arguments.
    if unsafe { libc::fsync(directory.output.as_raw_fd()) } != 0 {
        return Err(AppError::with_cause(
            "MERGE_PUBLISH_FAILED",
            "合并输出同步失败",
            std::io::Error::last_os_error().to_string(),
        ));
    }
    Ok(destination.output_path(output_name))
}

#[cfg(target_os = "macos")]
fn rename_no_replace(
    source_fd: libc::c_int,
    temp_path: &CString,
    output_fd: libc::c_int,
    output_name: &CString,
) -> libc::c_int {
    // SAFETY: both C strings and the destination directory descriptor are valid.
    unsafe {
        libc::renameatx_np(
            source_fd,
            temp_path.as_ptr(),
            output_fd,
            output_name.as_ptr(),
            libc::RENAME_EXCL,
        )
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn rename_no_replace(
    source_fd: libc::c_int,
    temp_path: &CString,
    output_fd: libc::c_int,
    output_name: &CString,
) -> libc::c_int {
    // SAFETY: both C strings and the destination directory descriptor are valid.
    unsafe {
        libc::renameat2(
            source_fd,
            temp_path.as_ptr(),
            output_fd,
            output_name.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
compile_error!("safe media publication requires atomic no-replace rename support");

fn output_changed_error(error: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "MERGE_OUTPUT_CHANGED",
        "合并输出目录在发布前发生变化",
        error.to_string(),
    )
}

fn is_videotoolbox_failure(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    const NON_ENCODER_FAILURES: [&str; 6] = [
        "permission denied",
        "no such file or directory",
        "no space left on device",
        "invalid data found when processing input",
        "device setup failed for decoder",
        "input/output error",
    ];
    if NON_ENCODER_FAILURES
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return false;
    }

    let mut previous_line_was_empty_encoder_context = false;
    for line in stderr.lines() {
        let parsed_context = parse_ffmpeg_encoder_context(line);
        if let Some(body) = parsed_context {
            if is_known_videotoolbox_diagnostic(body) {
                return true;
            }
        } else if previous_line_was_empty_encoder_context && is_known_videotoolbox_diagnostic(line)
        {
            return true;
        }
        previous_line_was_empty_encoder_context = parsed_context == Some("");
    }
    false
}

fn parse_ffmpeg_encoder_context(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("[h264_videotoolbox @ 0x")?;
    let bracket = rest.find(']')?;
    let address = &rest[..bracket];
    if address.is_empty()
        || address.len() > 16
        || !address.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let body = &rest[bracket + 1..];
    if body.is_empty() {
        Some("")
    } else {
        body.strip_prefix(' ')
    }
}

fn is_known_videotoolbox_diagnostic(body: &str) -> bool {
    matches!(
        body,
        "Error: cannot create compression session"
            | "Failed to create VTCompressionSession"
            | "VideoToolbox encoder failed"
            | "Hardware encoder setup failed"
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergeProgress {
    pub stage: String,
    pub percent: f64,
    pub terminal: bool,
}

struct ProgressTracker {
    expected_duration: f64,
    stage: String,
    percent: f64,
    last_ordinary: Option<Duration>,
}

impl ProgressTracker {
    fn new(expected_duration: f64) -> Self {
        Self {
            expected_duration,
            stage: String::new(),
            percent: 0.0,
            last_ordinary: None,
        }
    }

    fn stage(&mut self, stage: &str, _now: Duration, events: &mut Vec<MergeProgress>) {
        if self.stage != stage {
            self.stage = stage.into();
            events.push(MergeProgress {
                stage: self.stage.clone(),
                percent: self.percent,
                terminal: false,
            });
        }
    }

    fn line(&mut self, line: &str, now: Duration, events: &mut Vec<MergeProgress>) {
        let Some(micros) = line.strip_prefix("out_time_us=") else {
            return;
        };
        let Ok(micros) = micros.parse::<u64>() else {
            return;
        };
        if self.expected_duration <= 0.0 {
            return;
        }
        self.percent =
            ((micros as f64 / 1_000_000.0) / self.expected_duration * 100.0).clamp(0.0, 99.0);
        let should_emit = self
            .last_ordinary
            .map(|last| now.saturating_sub(last) >= Duration::from_millis(200))
            .unwrap_or(true);
        if should_emit {
            self.last_ordinary = Some(now);
            events.push(MergeProgress {
                stage: self.stage.clone(),
                percent: self.percent,
                terminal: false,
            });
        }
    }

    #[cfg(test)]
    fn terminal(
        &mut self,
        stage: &str,
        percent: f64,
        _now: Duration,
        events: &mut Vec<MergeProgress>,
    ) {
        self.stage = stage.into();
        self.percent = percent;
        events.push(MergeProgress {
            stage: self.stage.clone(),
            percent,
            terminal: true,
        });
    }
}

#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeResult {
    pub output_path: PathBuf,
}

pub fn probe_media(tools: &MediaTools, path: &Path) -> Result<MediaProbe, AppError> {
    probe_media_with_temp_directory(tools, path, None)
}

fn probe_media_with_temp_directory(
    tools: &MediaTools,
    path: &Path,
    temp_directory: Option<&TempDirectory>,
) -> Result<MediaProbe, AppError> {
    let mut command = tools.ffprobe_command();
    if let Some(temp_directory) = temp_directory {
        temp_directory.directory_path()?;
        set_child_working_directory(&mut command, temp_directory.directory.as_raw_fd());
    }
    let output = command
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
        return Err(AppError::with_cause(
            "FFPROBE_FAILED",
            "媒体文件无法读取",
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
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
    progress(MergeProgress {
        stage: "probing".into(),
        percent: 0.0,
        terminal: false,
    });
    let result = run_merge_inner(tools, request, cancellation, &mut progress);
    match &result {
        Ok(_) => progress(MergeProgress {
            stage: "completed".into(),
            percent: 100.0,
            terminal: true,
        }),
        Err(error) => progress(MergeProgress {
            stage: if error.code == "MERGE_CANCELLED" {
                "cancelled"
            } else {
                "failed"
            }
            .into(),
            percent: 0.0,
            terminal: true,
        }),
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
    let destination = ValidatedDestination::open(&request.series_root, &request.output_file_name)?;
    let validated_inputs = prepare_input_identities(&request.inputs)?;
    let normalized_inputs = validated_inputs
        .iter()
        .map(|validated| validated.input.clone())
        .collect::<Vec<_>>();
    let lines = concat_lines(&normalized_inputs)?;
    if cancellation.is_cancelled() {
        return Err(cancelled_error());
    }

    let mut probes = Vec::with_capacity(request.inputs.len());
    let mut sorted_inputs = validated_inputs.iter().collect::<Vec<_>>();
    sorted_inputs.sort_by_key(|input| input.episode_index);
    for input in sorted_inputs {
        if cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        probes.push(tools.probe_media(&input.path)?);
    }
    revalidate_input_identities(&validated_inputs)?;
    if !request.transcode_h264 && !can_stream_copy(&probes) {
        return Err(AppError::new(
            "MERGE_TRANSCODE_REQUIRED",
            "输入媒体参数不一致，需要开启 H.264 转码",
        ));
    }
    let expected_duration = probes
        .iter()
        .map(|probe| probe.duration_seconds)
        .sum::<f64>();
    let expected_audio = probes
        .first()
        .and_then(|probe| probe.audio.as_ref())
        .is_some();

    destination.verify_public_root_entry()?;
    let temp_dir = TempDirectory::create(&destination)?;
    temp_dir.write_concat(lines)?;

    let mut tracker = ProgressTracker::new(expected_duration);
    emit_stage(&mut tracker, "merging", progress);
    if request.transcode_h264 {
        revalidate_input_identities(&validated_inputs)?;
        let concat_path = temp_dir.concat_path()?;
        let temp_output = temp_dir.output_path()?;
        let first = run_ffmpeg(
            tools,
            &temp_dir,
            transcode_args(&concat_path, &temp_output, VideoEncoder::VideoToolbox),
            cancellation,
            &mut tracker,
            progress,
        )?;
        if !first.success {
            if !is_videotoolbox_failure(&first.stderr) {
                return Err(ffmpeg_error(first.stderr));
            }
            temp_dir.remove_output()?;
            emit_stage(&mut tracker, "software-fallback", progress);
            revalidate_input_identities(&validated_inputs)?;
            let concat_path = temp_dir.concat_path()?;
            let temp_output = temp_dir.output_path()?;
            let fallback = run_ffmpeg(
                tools,
                &temp_dir,
                transcode_args(&concat_path, &temp_output, VideoEncoder::LibX264),
                cancellation,
                &mut tracker,
                progress,
            )?;
            if !fallback.success {
                return Err(ffmpeg_error(fallback.stderr));
            }
        }
    } else {
        revalidate_input_identities(&validated_inputs)?;
        let concat_path = temp_dir.concat_path()?;
        let temp_output = temp_dir.output_path()?;
        let outcome = run_ffmpeg(
            tools,
            &temp_dir,
            stream_copy_args(&concat_path, &temp_output),
            cancellation,
            &mut tracker,
            progress,
        )?;
        if !outcome.success {
            return Err(ffmpeg_error(outcome.stderr));
        }
    }
    if cancellation.is_cancelled() {
        return Err(cancelled_error());
    }

    emit_stage(&mut tracker, "validating", progress);
    temp_dir.verify_identity()?;
    let temp_output = temp_dir.output_path()?;
    let output_probe = probe_media_with_temp_directory(tools, &temp_output, Some(&temp_dir))
        .map_err(|error| {
            AppError::with_cause(
                "MERGE_VALIDATION_FAILED",
                "合并输出验证失败",
                error.to_string(),
            )
        })?;
    validate_merged_output(&output_probe, expected_audio, expected_duration)?;

    let output_path = publish_output(
        &destination,
        &temp_dir,
        &request.output_file_name,
        request.conflict_policy,
    )?;
    Ok(MergeResult { output_path })
}

struct ValidatedInput {
    input: MergeInput,
    device: u64,
    inode: u64,
}

impl std::ops::Deref for ValidatedInput {
    type Target = MergeInput;

    fn deref(&self) -> &Self::Target {
        &self.input
    }
}

fn prepare_input_identities(inputs: &[MergeInput]) -> Result<Vec<ValidatedInput>, AppError> {
    let mut validated = Vec::with_capacity(inputs.len());
    for input in inputs {
        let canonical_path = fs::canonicalize(&input.path).map_err(input_changed_error)?;
        let metadata = fs::metadata(&canonical_path).map_err(input_changed_error)?;
        let modified = metadata_modified_nanos(&metadata)?;
        if !metadata.is_file()
            || metadata.len() != input.size
            || modified != input.modified_unix_nanos
        {
            return Err(input_changed_error("snapshot mismatch"));
        }
        validated.push(ValidatedInput {
            input: MergeInput {
                path: canonical_path,
                ..input.clone()
            },
            device: metadata.dev(),
            inode: metadata.ino(),
        });
    }
    Ok(validated)
}

fn revalidate_input_identities(inputs: &[ValidatedInput]) -> Result<(), AppError> {
    for validated in inputs {
        let metadata = fs::metadata(&validated.path).map_err(input_changed_error)?;
        if !metadata.is_file()
            || metadata.dev() != validated.device
            || metadata.ino() != validated.inode
            || metadata.len() != validated.size
            || metadata_modified_nanos(&metadata)? != validated.modified_unix_nanos
        {
            return Err(input_changed_error("identity mismatch"));
        }
    }
    Ok(())
}

fn metadata_modified_nanos(metadata: &fs::Metadata) -> Result<u128, AppError> {
    metadata
        .modified()
        .and_then(|value| {
            value
                .duration_since(UNIX_EPOCH)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
        })
        .map(|duration| duration.as_nanos())
        .map_err(input_changed_error)
}

fn input_changed_error(error: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "MEDIA_INPUT_CHANGED",
        "合并输入已移动或发生变化",
        error.to_string(),
    )
}

fn validate_merged_output(
    output: &MediaProbe,
    expected_audio: bool,
    expected_duration: f64,
) -> Result<(), AppError> {
    let duration_tolerance = (expected_duration * 0.01).max(1.0);
    if output.audio.is_some() != expected_audio
        || (output.duration_seconds - expected_duration).abs() > duration_tolerance
    {
        return Err(AppError::new("MERGE_VALIDATION_FAILED", "合并输出验证失败"));
    }
    Ok(())
}

struct TempDirectory {
    parent: OwnedFd,
    directory: OwnedFd,
    entry: CString,
    device: u64,
    inode: u64,
}

impl TempDirectory {
    fn create(destination: &ValidatedDestination) -> Result<Self, AppError> {
        destination.verify_public_root_entry()?;
        let parent = duplicate_fd(destination.root.as_raw_fd()).map_err(temp_create_error)?;
        for _ in 0..128 {
            let entry = random_temp_entry()?;
            // SAFETY: parent is a held series-root descriptor and entry is a valid C string.
            let mkdir_result = unsafe { libc::mkdirat(parent.as_raw_fd(), entry.as_ptr(), 0o700) };
            if mkdir_result != 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    continue;
                }
                return Err(temp_create_error(error));
            }
            // SAFETY: parent and entry are valid; the returned descriptor is checked.
            let raw_directory = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    entry.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if raw_directory < 0 {
                // SAFETY: unlink is constrained to the held series-root descriptor.
                let _ = unsafe {
                    libc::unlinkat(parent.as_raw_fd(), entry.as_ptr(), libc::AT_REMOVEDIR)
                };
                return Err(temp_create_error(std::io::Error::last_os_error()));
            }
            // SAFETY: raw_directory is newly opened and owned by this function.
            let directory = unsafe { OwnedFd::from_raw_fd(raw_directory) };
            // SAFETY: directory is live and mode 0700 is required for private task state.
            if unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } != 0 {
                // SAFETY: unlink is constrained to the held series-root descriptor.
                let _ = unsafe {
                    libc::unlinkat(parent.as_raw_fd(), entry.as_ptr(), libc::AT_REMOVEDIR)
                };
                return Err(temp_create_error(std::io::Error::last_os_error()));
            }
            let (device, inode) = temp_directory_identity(directory.as_raw_fd())?;
            if device != destination.device {
                // SAFETY: unlink is constrained to the held series-root descriptor.
                let _ = unsafe {
                    libc::unlinkat(parent.as_raw_fd(), entry.as_ptr(), libc::AT_REMOVEDIR)
                };
                return Err(temp_create_error("temporary directory device mismatch"));
            }
            let temp = Self {
                parent,
                directory,
                entry,
                device,
                inode,
            };
            temp.verify_identity()?;
            return Ok(temp);
        }
        Err(temp_create_error(
            "could not create a unique temporary directory",
        ))
    }

    fn verify_identity(&self) -> Result<(), AppError> {
        let (held_device, held_inode) = temp_directory_identity(self.directory.as_raw_fd())?;
        if held_device != self.device || held_inode != self.inode {
            return Err(temp_changed_error(
                "held temporary directory identity changed",
            ));
        }

        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: parent and entry are valid and stat points to writable storage.
        let result = unsafe {
            libc::fstatat(
                self.parent.as_raw_fd(),
                self.entry.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            return Err(temp_changed_error(std::io::Error::last_os_error()));
        }
        // SAFETY: fstatat succeeded and initialized stat.
        let stat = unsafe { stat.assume_init() };
        if stat.st_mode & libc::S_IFMT != libc::S_IFDIR
            || stat.st_dev as u64 != self.device
            || stat.st_ino != self.inode
        {
            return Err(temp_changed_error("temporary directory identity changed"));
        }
        Ok(())
    }

    fn write_concat(&self, lines: Vec<String>) -> Result<(), AppError> {
        self.verify_identity()?;
        let entry = temporary_concat_entry();
        // SAFETY: directory and entry are valid; the returned descriptor is checked.
        let raw_file = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                entry.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if raw_file < 0 {
            return Err(merge_io_error(std::io::Error::last_os_error()));
        }
        // SAFETY: raw_file is newly opened and owned by this function.
        let mut concat_file = unsafe { fs::File::from_raw_fd(raw_file) };
        for line in lines {
            writeln!(concat_file, "{line}").map_err(merge_io_error)?;
        }
        concat_file.sync_all().map_err(merge_io_error)?;
        Ok(())
    }

    fn concat_path(&self) -> Result<PathBuf, AppError> {
        self.directory_path()?;
        Ok(PathBuf::from("concat.txt"))
    }

    fn output_path(&self) -> Result<PathBuf, AppError> {
        self.directory_path()?;
        Ok(PathBuf::from("merged.mp4"))
    }

    fn directory_path(&self) -> Result<PathBuf, AppError> {
        self.verify_identity()?;
        let path = path_from_fd(self.directory.as_raw_fd())?;
        let metadata = fs::symlink_metadata(&path).map_err(temp_changed_error)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.dev() != self.device
            || metadata.ino() != self.inode
        {
            return Err(temp_changed_error(
                "temporary directory path identity changed",
            ));
        }
        Ok(path)
    }

    fn remove_output(&self) -> Result<(), AppError> {
        self.verify_identity()?;
        let entry = temporary_output_entry();
        // SAFETY: directory and entry are valid and unlink is descriptor-relative.
        let result = unsafe { libc::unlinkat(self.directory.as_raw_fd(), entry.as_ptr(), 0) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(merge_io_error(error));
            }
        }
        Ok(())
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let entry_is_current = self.verify_identity().is_ok();
        for name in ["concat.txt", "merged.mp4"] {
            let entry = CString::new(name).expect("fixed temp filename has no NUL");
            // SAFETY: directory and entry are valid; failures are best-effort cleanup only.
            let _ = unsafe { libc::unlinkat(self.directory.as_raw_fd(), entry.as_ptr(), 0) };
        }
        if entry_is_current {
            // SAFETY: identity was just checked and unlink is constrained to the held series root.
            let _ = unsafe {
                libc::unlinkat(
                    self.parent.as_raw_fd(),
                    self.entry.as_ptr(),
                    libc::AT_REMOVEDIR,
                )
            };
        }
    }
}

fn random_temp_entry() -> Result<CString, AppError> {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let random = random_u128();
    CString::new(format!(
        ".hongguo-merge-{}-{sequence}-{random:032x}",
        std::process::id()
    ))
    .map_err(|_| temp_create_error("temporary entry contains NUL"))
}

fn random_u128() -> u128 {
    let mut bytes = [0_u8; 16];
    if fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_ok()
    {
        return u128::from_ne_bytes(bytes);
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    nanos ^ ((std::process::id() as u128) << 64) ^ TEMP_SEQUENCE.load(Ordering::Relaxed) as u128
}

fn duplicate_fd(fd: libc::c_int) -> Result<OwnedFd, std::io::Error> {
    // SAFETY: fcntl duplicates the live descriptor and returns a new owned descriptor on success.
    let raw = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: raw is a newly duplicated descriptor owned by this function.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

fn set_child_working_directory(command: &mut Command, directory_fd: libc::c_int) {
    // SAFETY: the closure runs in the child immediately before exec and only calls async-signal-safe fchdir.
    unsafe {
        command.pre_exec(move || {
            if libc::fchdir(directory_fd) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(target_os = "macos")]
fn path_from_fd(fd: libc::c_int) -> Result<PathBuf, AppError> {
    let mut buffer = vec![0 as libc::c_char; libc::PATH_MAX as usize];
    // SAFETY: fd is live, F_GETPATH writes a NUL-terminated path into a PATH_MAX buffer on macOS.
    let result = unsafe { libc::fcntl(fd, libc::F_GETPATH, buffer.as_mut_ptr()) };
    if result != 0 {
        return Err(temp_changed_error(std::io::Error::last_os_error()));
    }
    // SAFETY: successful F_GETPATH writes a NUL-terminated string into the buffer.
    let path = unsafe { CStr::from_ptr(buffer.as_ptr()) };
    Ok(PathBuf::from(OsString::from_vec(path.to_bytes().to_vec())))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn path_from_fd(fd: libc::c_int) -> Result<PathBuf, AppError> {
    fs::read_link(format!("/proc/self/fd/{fd}")).map_err(temp_changed_error)
}

fn temporary_concat_entry() -> CString {
    CString::new("concat.txt").expect("fixed temp filename has no NUL")
}

fn temporary_output_entry() -> CString {
    CString::new("merged.mp4").expect("fixed temp filename has no NUL")
}

fn temp_directory_identity(fd: libc::c_int) -> Result<(u64, u64), AppError> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fd is live and stat points to writable storage.
    let result = unsafe { libc::fstat(fd, stat.as_mut_ptr()) };
    if result != 0 {
        return Err(temp_changed_error(std::io::Error::last_os_error()));
    }
    // SAFETY: fstat returned success and initialized stat.
    let stat = unsafe { stat.assume_init() };
    Ok((stat.st_dev as u64, stat.st_ino))
}

fn temp_create_error(error: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "MERGE_TEMP_FAILED",
        "无法创建唯一合并临时目录",
        error.to_string(),
    )
}

fn temp_changed_error(error: impl std::fmt::Display) -> AppError {
    AppError::with_cause(
        "MERGE_TEMP_CHANGED",
        "合并临时目录在执行期间发生变化",
        error.to_string(),
    )
}

struct ProcessOutcome {
    success: bool,
    stderr: String,
}

fn run_ffmpeg(
    tools: &MediaTools,
    temp_directory: &TempDirectory,
    args: Vec<String>,
    cancellation: &CancellationToken,
    tracker: &mut ProgressTracker,
    progress: &mut dyn FnMut(MergeProgress),
) -> Result<ProcessOutcome, AppError> {
    temp_directory.verify_identity()?;
    temp_directory.directory_path()?;
    let mut command = tools.ffmpeg_command();
    set_child_working_directory(&mut command, temp_directory.directory.as_raw_fd());
    let mut child = command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            AppError::with_cause("FFMPEG_FAILED", "无法运行打包的合并工具", error.to_string())
        })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ffmpeg_error(String::new()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ffmpeg_error(String::new()))?;
    let (line_sender, line_receiver) = mpsc::channel();
    let stdout_thread = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line_sender.send(line).is_err() {
                break;
            }
        }
    });
    let stderr_thread = thread::spawn(move || {
        let mut stderr = stderr;
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let start = Instant::now();

    let status = loop {
        drain_progress(&line_receiver, tracker, start.elapsed(), progress);
        if cancellation.is_cancelled() {
            terminate_and_wait(&mut child);
            let _ = stdout_thread.join();
            let _ = stderr_thread.join();
            return Err(cancelled_error());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                terminate_and_wait(&mut child);
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Err(AppError::with_cause(
                    "FFMPEG_FAILED",
                    "合并子进程状态异常",
                    error.to_string(),
                ));
            }
        }
    };
    let _ = stdout_thread.join();
    drain_progress(&line_receiver, tracker, start.elapsed(), progress);
    let stderr = stderr_thread.join().unwrap_or_default();
    Ok(ProcessOutcome {
        success: status.success(),
        stderr,
    })
}

fn drain_progress(
    receiver: &mpsc::Receiver<String>,
    tracker: &mut ProgressTracker,
    elapsed: Duration,
    progress: &mut dyn FnMut(MergeProgress),
) {
    let mut events = Vec::new();
    for line in receiver.try_iter() {
        tracker.line(&line, elapsed, &mut events);
    }
    for event in events {
        progress(event);
    }
}

fn emit_stage(tracker: &mut ProgressTracker, stage: &str, progress: &mut dyn FnMut(MergeProgress)) {
    let mut events = Vec::new();
    tracker.stage(stage, Duration::ZERO, &mut events);
    for event in events {
        progress(event);
    }
}

fn terminate_and_wait(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn ffmpeg_error(stderr: String) -> AppError {
    AppError::with_cause("FFMPEG_FAILED", "合并子进程执行失败", stderr)
}

fn cancelled_error() -> AppError {
    AppError::new("MERGE_CANCELLED", "合并任务已取消")
}

fn merge_io_error(error: impl std::fmt::Display) -> AppError {
    AppError::with_cause("MERGE_IO", "合并文件操作失败", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::tools::MediaTools;
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct FixtureDir(PathBuf);

    impl FixtureDir {
        fn new() -> Self {
            let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "hongguo-merge-contract-test-{}-{}-{sequence}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write_executable(&self, name: &str, body: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        fn tools(&self, ffmpeg: &str, ffprobe: &str) -> MediaTools {
            self.write_executable("ffmpeg", ffmpeg);
            self.write_executable("ffprobe", ffprobe);
            MediaTools::from_resource_root(&self.0).unwrap()
        }

        fn merge_input(&self, index: u32, name: &str) -> MergeInput {
            let path = self.0.join(name);
            fs::write(&path, format!("source-{index}")).unwrap();
            let metadata = fs::metadata(&path).unwrap();
            let modified_unix_nanos = metadata
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            MergeInput {
                episode_index: index,
                path,
                size: metadata.len(),
                modified_unix_nanos,
            }
        }
    }

    impl Drop for FixtureDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn probe(video_codec: &str, height: u32, audio_codec: &str, sample_rate: u32) -> MediaProbe {
        MediaProbe {
            video: StreamSignature {
                codec_name: video_codec.into(),
                width: Some(1920),
                height: Some(height),
                frame_rate: Some("30/1".into()),
                time_base: Some("1/15360".into()),
                sample_rate: None,
                channels: None,
            },
            audio: Some(StreamSignature {
                codec_name: audio_codec.into(),
                width: None,
                height: None,
                frame_rate: None,
                time_base: Some("1/48000".into()),
                sample_rate: Some(sample_rate),
                channels: Some(2),
            }),
            duration_seconds: 1.0,
        }
    }

    fn input(index: u32, path: &str) -> MergeInput {
        MergeInput {
            episode_index: index,
            path: PathBuf::from(path),
            size: 10,
            modified_unix_nanos: 20,
        }
    }

    fn merge_request(fixture: &FixtureDir, inputs: Vec<MergeInput>) -> MergeRequest {
        MergeRequest {
            series_root: fixture.0.clone(),
            output_file_name: "全集.mp4".into(),
            inputs,
            transcode_h264: false,
            conflict_policy: MergeConflictPolicy::FailIfExists,
        }
    }

    fn probe_script(output_duration: &str) -> String {
        format!(
            r#"#!/bin/sh
last=""
for arg in "$@"; do last="$arg"; done
case "$last" in
  *two.mp4) codec=hevc ;;
  *) codec=h264 ;;
esac
case "$last" in
  *merged.mp4) duration={output_duration} ;;
  *) duration=1.0 ;;
esac
printf '{{"streams":[{{"codec_type":"video","codec_name":"%s","width":16,"height":16,"r_frame_rate":"1/1","time_base":"1/16384"}},{{"codec_type":"audio","codec_name":"aac","sample_rate":"48000","channels":1,"time_base":"1/48000"}}],"format":{{"duration":"%s"}}}}' "$codec" "$duration"
"#
        )
    }

    fn successful_ffmpeg_script() -> &'static str {
        r#"#!/bin/sh
last=""
for arg in "$@"; do last="$arg"; done
printf 'fixture-output' > "$last"
printf 'out_time_us=1000000\nprogress=end\n'
"#
    }

    #[test]
    fn stream_copy_requires_matching_video_and_audio_signatures() {
        // Production mutation caught: comparing only video codec or ignoring optional audio.
        assert!(can_stream_copy(&[
            probe("h264", 1080, "aac", 48000),
            probe("h264", 1080, "aac", 48000),
        ]));
        assert!(!can_stream_copy(&[
            probe("h264", 1080, "aac", 48000),
            probe("hevc", 1080, "aac", 48000),
        ]));
        assert!(!can_stream_copy(&[
            probe("h264", 1080, "aac", 48000),
            probe("h264", 1080, "aac", 44100),
        ]));
        assert!(!can_stream_copy(&[
            probe("h264", 1080, "aac", 48000),
            probe("h264", 720, "aac", 48000),
        ]));
        let mut without_audio = probe("h264", 1080, "aac", 48000);
        without_audio.audio = None;
        assert!(!can_stream_copy(&[
            probe("h264", 1080, "aac", 48000),
            without_audio,
        ]));
    }

    #[test]
    fn inputs_are_sorted_by_episode_index_before_concat_file_is_written() {
        // Production mutation caught: preserving caller order instead of episode order.
        let lines = concat_lines(&[input(2, "b.mp4"), input(1, "a.mp4")]).unwrap();
        assert_eq!(lines, vec!["file 'a.mp4'", "file 'b.mp4'"]);
    }

    #[test]
    fn concat_rejects_zero_duplicate_and_unsafe_paths() {
        // Production mutation caught: emitting ambiguous indices or injectable concat directives.
        assert_eq!(
            concat_lines(&[input(0, "zero.mp4")]).unwrap_err().code,
            "MERGE_INPUT_INVALID"
        );
        assert_eq!(
            concat_lines(&[input(1, "a.mp4"), input(1, "b.mp4")])
                .unwrap_err()
                .code,
            "MERGE_INPUT_DUPLICATE_INDEX"
        );
        for path in ["a'b.mp4", "a\nb.mp4", "a\tb.mp4"] {
            assert_eq!(
                concat_lines(&[input(1, path)]).unwrap_err().code,
                "MERGE_INPUT_UNSAFE_PATH"
            );
        }
    }

    #[test]
    fn command_builders_emit_exact_stream_copy_and_h264_argv() {
        // Production mutation caught: shell-like combined arguments or wrong encoder/audio policy.
        let concat = PathBuf::from("/tmp/job/concat.txt");
        let output = PathBuf::from("/tmp/job/merged.mp4");
        assert_eq!(
            stream_copy_args(&concat, &output),
            vec![
                "-hide_banner",
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
                "/tmp/job/concat.txt",
                "-map",
                "0:v:0",
                "-map",
                "0:a:0?",
                "-c",
                "copy",
                "-movflags",
                "+faststart",
                "-progress",
                "pipe:1",
                "-nostats",
                "/tmp/job/merged.mp4"
            ]
        );
        assert_eq!(
            transcode_args(&concat, &output, VideoEncoder::VideoToolbox),
            vec![
                "-hide_banner",
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
                "/tmp/job/concat.txt",
                "-map",
                "0:v:0",
                "-map",
                "0:a:0?",
                "-c:v",
                "h264_videotoolbox",
                "-c:a",
                "aac",
                "-movflags",
                "+faststart",
                "-progress",
                "pipe:1",
                "-nostats",
                "/tmp/job/merged.mp4"
            ]
        );
        assert_eq!(
            transcode_args(&concat, &output, VideoEncoder::LibX264),
            vec![
                "-hide_banner",
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
                "/tmp/job/concat.txt",
                "-map",
                "0:v:0",
                "-map",
                "0:a:0?",
                "-c:v",
                "libx264",
                "-c:a",
                "aac",
                "-movflags",
                "+faststart",
                "-progress",
                "pipe:1",
                "-nostats",
                "/tmp/job/merged.mp4"
            ]
        );
    }

    #[test]
    fn ffprobe_json_requires_one_video_and_finite_non_negative_duration() {
        // Production mutation caught: accepting zero/multiple videos or NaN/negative duration.
        let valid = json!({
            "streams": [
                {"codec_type":"video","codec_name":"h264","width":1920,"height":1080,
                 "r_frame_rate":"30/1","time_base":"1/15360"},
                {"codec_type":"audio","codec_name":"aac","sample_rate":"48000",
                 "channels":2,"time_base":"1/48000"}
            ],
            "format": {"duration":"1.25"}
        });
        let parsed = parse_probe_json(valid.to_string().as_bytes()).unwrap();
        assert_eq!(parsed.video.codec_name, "h264");
        assert_eq!(parsed.audio.unwrap().sample_rate, Some(48000));
        assert_eq!(parsed.duration_seconds, 1.25);

        for invalid in [
            json!({"streams":[], "format":{"duration":"1"}}),
            json!({"streams":[
                {"codec_type":"video","codec_name":"h264"},
                {"codec_type":"video","codec_name":"h264"}
            ], "format":{"duration":"1"}}),
            json!({"streams":[{"codec_type":"video","codec_name":"h264"}],
                   "format":{"duration":"NaN"}}),
            json!({"streams":[{"codec_type":"video","codec_name":"h264"}],
                   "format":{"duration":"-0.1"}}),
        ] {
            assert_eq!(
                parse_probe_json(invalid.to_string().as_bytes())
                    .unwrap_err()
                    .code,
                "FFPROBE_INVALID"
            );
        }
    }

    #[test]
    fn malformed_ffprobe_error_does_not_serialize_raw_diagnostic() {
        // Production mutation caught: copying ffprobe JSON/path details into public diagnostics.
        let error = parse_probe_json(br#"{/secret/download/account.mp4"#).unwrap_err();
        let serialized = serde_json::to_string(&error).unwrap();
        assert_eq!(error.code, "FFPROBE_INVALID");
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("account.mp4"));
    }

    #[test]
    fn output_name_is_a_single_mp4_component_contained_in_series_root() {
        // Production mutation caught: allowing traversal/absolute output outside the selected series.
        let fixture = FixtureDir::new();
        let output = validated_output_path(&fixture.0, "全集.mp4").unwrap();
        assert_eq!(
            output,
            fs::canonicalize(&fixture.0)
                .unwrap()
                .join("合并视频")
                .join("全集.mp4")
        );
        for unsafe_name in [
            "../escape.mp4",
            "/tmp/escape.mp4",
            "nested/out.mp4",
            "x.txt",
        ] {
            assert_eq!(
                validated_output_path(&fixture.0, unsafe_name)
                    .unwrap_err()
                    .code,
                "MERGE_OUTPUT_INVALID"
            );
        }
    }

    #[test]
    fn fallback_allowlist_accepts_only_explicit_videotoolbox_failures() {
        // Production mutation caught: retrying libx264 for input, permission, or disk errors.
        assert!(is_videotoolbox_failure(
            "[h264_videotoolbox @ 0x123] Error: cannot create compression session"
        ));
        assert!(is_videotoolbox_failure(
            "[h264_videotoolbox @ 0x123]\nFailed to create VTCompressionSession"
        ));
        assert!(!is_videotoolbox_failure(
            " [h264_videotoolbox @ 0x123] Error: cannot create compression session"
        ));
        assert!(!is_videotoolbox_failure(
            "[h264_videotoolbox @ not-hex] Error: cannot create compression session"
        ));
        assert!(!is_videotoolbox_failure(
            "[other_h264_videotoolbox @ 0x123] Error: cannot create compression session"
        ));
        for stderr in [
            "No such file or directory",
            "Permission denied",
            "No space left on device",
            "Invalid data found when processing input",
            "No device available for decoder: device setup failed for decoder on input stream #0:0",
        ] {
            assert!(!is_videotoolbox_failure(stderr));
        }
    }

    #[test]
    fn progress_parser_throttles_ordinary_updates_but_not_stage_or_terminal_events() {
        // Production mutation caught: event flooding or suppressing stage/terminal changes.
        let mut tracker = ProgressTracker::new(10.0);
        let mut events = Vec::new();
        tracker.stage("merging", Duration::ZERO, &mut events);
        tracker.line(
            "out_time_us=1000000",
            Duration::from_millis(10),
            &mut events,
        );
        tracker.line(
            "out_time_us=2000000",
            Duration::from_millis(100),
            &mut events,
        );
        tracker.line(
            "out_time_us=3000000",
            Duration::from_millis(211),
            &mut events,
        );
        tracker.stage("validating", Duration::from_millis(212), &mut events);
        tracker.terminal("completed", 100.0, Duration::from_millis(213), &mut events);

        assert_eq!(
            events
                .iter()
                .map(|event| (event.stage.as_str(), event.percent as u32, event.terminal))
                .collect::<Vec<_>>(),
            vec![
                ("merging", 0, false),
                ("merging", 10, false),
                ("merging", 30, false),
                ("validating", 30, false),
                ("completed", 100, true),
            ]
        );
    }

    #[test]
    fn incompatible_copy_fails_before_any_temp_or_output_is_created() {
        // Production mutation caught: creating output/temp before copy compatibility is known.
        let fixture = FixtureDir::new();
        let tools = fixture.tools(successful_ffmpeg_script(), &probe_script("2.0"));
        let request = merge_request(
            &fixture,
            vec![
                fixture.merge_input(1, "one.mp4"),
                fixture.merge_input(2, "two.mp4"),
            ],
        );

        let error = run_merge(&tools, request, &CancellationToken::new(), |_| {}).unwrap_err();

        assert_eq!(error.code, "MERGE_TRANSCODE_REQUIRED");
        assert!(!fixture.0.join("合并视频").exists());
        assert!(!fs::read_dir(&fixture.0)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".hongguo-merge-")));
    }

    #[test]
    fn validation_failure_cleans_temp_without_deleting_sources_or_publishing_output() {
        // Production mutation caught: publishing before ffprobe validation or deleting source MP4s.
        let fixture = FixtureDir::new();
        let tools = fixture.tools(successful_ffmpeg_script(), &probe_script("99.0"));
        let source = fixture.merge_input(1, "one.mp4");
        let source_bytes = fs::read(&source.path).unwrap();

        let error = run_merge(
            &tools,
            merge_request(&fixture, vec![source.clone()]),
            &CancellationToken::new(),
            |_| {},
        )
        .unwrap_err();

        assert_eq!(error.code, "MERGE_VALIDATION_FAILED");
        assert_eq!(fs::read(&source.path).unwrap(), source_bytes);
        assert!(!fixture.0.join("合并视频").join("全集.mp4").exists());
        assert!(!fs::read_dir(&fixture.0)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".hongguo-merge-")));
    }

    #[test]
    fn cancellation_kills_child_and_removes_only_job_temp_directory() {
        // Production mutation caught: leaving child/temp behind or deleting original downloads.
        let fixture = FixtureDir::new();
        let tools = fixture.tools(
            r#"#!/bin/sh
last=""
for arg in "$@"; do last="$arg"; done
printf 'partial' > "$last"
exec /bin/sleep 30
"#,
            &probe_script("1.0"),
        );
        let source = fixture.merge_input(1, "one.mp4");
        let unrelated = fixture.0.join("keep.txt");
        fs::write(&unrelated, b"keep").unwrap();
        let cancellation = CancellationToken::new();
        let trigger = cancellation.clone();
        let cancel_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            trigger.cancel();
        });

        let error = run_merge(
            &tools,
            merge_request(&fixture, vec![source.clone()]),
            &cancellation,
            |_| {},
        )
        .unwrap_err();
        cancel_thread.join().unwrap();

        assert_eq!(error.code, "MERGE_CANCELLED");
        assert!(source.path.exists());
        assert_eq!(fs::read(&unrelated).unwrap(), b"keep");
        assert!(!fs::read_dir(&fixture.0)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".hongguo-merge-")));
    }

    #[test]
    fn successful_validation_publishes_once_and_default_does_not_overwrite() {
        // Production mutation caught: overwriting by default or retaining temp after atomic publish.
        let fixture = FixtureDir::new();
        let tools = fixture.tools(successful_ffmpeg_script(), &probe_script("1.0"));
        let source = fixture.merge_input(1, "one.mp4");
        let request = merge_request(&fixture, vec![source]);

        let result = run_merge(&tools, request.clone(), &CancellationToken::new(), |_| {}).unwrap();
        assert_eq!(fs::read(&result.output_path).unwrap(), b"fixture-output");

        let error = run_merge(&tools, request, &CancellationToken::new(), |_| {}).unwrap_err();
        assert_eq!(error.code, "MERGE_OUTPUT_EXISTS");
        assert_eq!(fs::read(&result.output_path).unwrap(), b"fixture-output");
    }

    #[test]
    fn hardware_encoder_failure_retries_once_but_permission_error_does_not() {
        // Production mutation caught: skipping fallback or retrying arbitrary process failures.
        let hardware_fixture = FixtureDir::new();
        let hardware_log = hardware_fixture.0.join("calls.log");
        let hardware_script = format!(
            r#"#!/bin/sh
printf 'BEGIN\n' >> '{}'
last=""
hardware=0
for arg in "$@"; do
  printf '%s\n' "$arg" >> '{}'
  last="$arg"
  if [ "$arg" = h264_videotoolbox ]; then hardware=1; fi
done
if [ "$hardware" = 1 ]; then
  printf '[h264_videotoolbox @ 0x123] Failed to create VTCompressionSession\n' >&2
  exit 1
fi
printf 'fixture-output' > "$last"
exit 0
"#,
            hardware_log.display(),
            hardware_log.display()
        );
        let tools = hardware_fixture.tools(&hardware_script, &probe_script("1.0"));
        let mut request = merge_request(
            &hardware_fixture,
            vec![hardware_fixture.merge_input(1, "one.mp4")],
        );
        request.transcode_h264 = true;

        run_merge(&tools, request, &CancellationToken::new(), |_| {}).unwrap();

        let log = fs::read_to_string(&hardware_log).unwrap();
        assert_eq!(log.matches("BEGIN").count(), 2);
        assert!(log.lines().any(|line| line == "h264_videotoolbox"));
        assert!(log.lines().any(|line| line == "libx264"));

        let permission_fixture = FixtureDir::new();
        let permission_log = permission_fixture.0.join("calls.log");
        let permission_script = format!(
            "#!/bin/sh\nprintf 'BEGIN\\n' >> '{}'\nprintf 'Permission denied\\n' >&2\nexit 1\n",
            permission_log.display()
        );
        let tools = permission_fixture.tools(&permission_script, &probe_script("1.0"));
        let mut request = merge_request(
            &permission_fixture,
            vec![permission_fixture.merge_input(1, "one.mp4")],
        );
        request.transcode_h264 = true;

        let error = run_merge(&tools, request, &CancellationToken::new(), |_| {}).unwrap_err();

        assert_eq!(error.code, "FFMPEG_FAILED");
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains("Permission denied"));
        assert_eq!(
            fs::read_to_string(permission_log)
                .unwrap()
                .matches("BEGIN")
                .count(),
            1
        );
    }

    #[test]
    fn permission_diagnostic_cannot_spoof_hardware_fallback_with_allowlist_text() {
        // Production mutation caught: classifying arbitrary full-stderr substrings as encoder failures.
        let fixture = FixtureDir::new();
        let calls = fixture.0.join("calls.log");
        let ffmpeg = format!(
            "#!/bin/sh\nprintf 'BEGIN\\n' >> '{}'\nprintf 'Permission denied opening /downloads/cannot create compression session/input.mp4\\n' >&2\nexit 1\n",
            calls.display()
        );
        let tools = fixture.tools(&ffmpeg, &probe_script("1.0"));
        let mut request = merge_request(&fixture, vec![fixture.merge_input(1, "one.mp4")]);
        request.transcode_h264 = true;

        let error = run_merge(&tools, request, &CancellationToken::new(), |_| {}).unwrap_err();

        assert_eq!(error.code, "FFMPEG_FAILED");
        assert_eq!(
            fs::read_to_string(calls).unwrap().matches("BEGIN").count(),
            1
        );
    }

    #[test]
    fn filename_cannot_spoof_bracketed_hardware_encoder_context() {
        // Production mutation caught: accepting bracket-like encoder text embedded in a normal line/path.
        let fixture = FixtureDir::new();
        let calls = fixture.0.join("calls.log");
        let ffmpeg = format!(
            "#!/bin/sh\nprintf 'BEGIN\\n' >> '{}'\nprintf 'Could not open input /downloads/[h264_videotoolbox @ 0x123] cannot create compression session.mp4\\n' >&2\nexit 1\n",
            calls.display()
        );
        let tools = fixture.tools(&ffmpeg, &probe_script("1.0"));
        let mut request = merge_request(&fixture, vec![fixture.merge_input(1, "one.mp4")]);
        request.transcode_h264 = true;

        let error = run_merge(&tools, request, &CancellationToken::new(), |_| {}).unwrap_err();

        assert_eq!(error.code, "FFMPEG_FAILED");
        assert_eq!(
            fs::read_to_string(calls).unwrap().matches("BEGIN").count(),
            1
        );
    }

    #[test]
    fn input_replaced_after_probe_is_rejected_before_ffmpeg_spawn() {
        // Production mutation caught: trusting only the pre-probe size/mtime snapshot.
        let fixture = FixtureDir::new();
        let calls = fixture.0.join("ffmpeg-calls.log");
        let ffmpeg = format!(
            r#"#!/bin/sh
printf 'BEGIN\n' >> '{}'
last=""
for arg in "$@"; do last="$arg"; done
printf 'fixture-output' > "$last"
"#,
            calls.display()
        );
        let ffprobe = r#"#!/bin/sh
last=""
for arg in "$@"; do last="$arg"; done
printf '{"streams":[{"codec_type":"video","codec_name":"h264","width":16,"height":16,"r_frame_rate":"1/1","time_base":"1/16384"}],"format":{"duration":"1.0"}}'
case "$last" in
  *one.mp4)
    printf 'replaced' > "$last.replacement"
    touch -r "$last" "$last.replacement"
    mv "$last.replacement" "$last"
    ;;
esac
"#;
        let tools = fixture.tools(&ffmpeg, ffprobe);
        let source = fixture.merge_input(1, "one.mp4");
        let original_size = source.size;
        let original_modified = source.modified_unix_nanos;

        let error = run_merge(
            &tools,
            merge_request(&fixture, vec![source.clone()]),
            &CancellationToken::new(),
            |_| {},
        )
        .unwrap_err();

        assert_eq!(error.code, "MEDIA_INPUT_CHANGED");
        assert!(!calls.exists());
        assert!(source.path.exists());
        let replaced_metadata = fs::metadata(&source.path).unwrap();
        assert_eq!(replaced_metadata.len(), original_size);
        assert_eq!(
            replaced_metadata
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            original_modified
        );
        assert!(!fixture.0.join("合并视频").join("全集.mp4").exists());
    }

    #[test]
    fn input_replaced_by_hardware_attempt_is_rejected_before_software_retry() {
        // Production mutation caught: omitting identity revalidation before the fallback spawn.
        let fixture = FixtureDir::new();
        let source = fixture.merge_input(1, "one.mp4");
        let calls = fixture.0.join("ffmpeg-calls.log");
        let ffmpeg = format!(
            r#"#!/bin/sh
printf 'BEGIN\n' >> '{}'
hardware=0
for arg in "$@"; do
  if [ "$arg" = h264_videotoolbox ]; then hardware=1; fi
done
if [ "$hardware" = 1 ]; then
  printf 'replaced' > '{}.replacement'
  touch -r '{}' '{}.replacement'
  mv '{}.replacement' '{}'
  printf '[h264_videotoolbox @ 0x123] Failed to create VTCompressionSession\n' >&2
  exit 1
fi
exit 99
"#,
            calls.display(),
            source.path.display(),
            source.path.display(),
            source.path.display(),
            source.path.display(),
            source.path.display(),
        );
        let tools = fixture.tools(&ffmpeg, &probe_script("1.0"));
        let mut request = merge_request(&fixture, vec![source.clone()]);
        request.transcode_h264 = true;

        let error = run_merge(&tools, request, &CancellationToken::new(), |_| {}).unwrap_err();

        assert_eq!(error.code, "MEDIA_INPUT_CHANGED");
        assert_eq!(
            fs::read_to_string(calls).unwrap().matches("BEGIN").count(),
            1
        );
        assert!(source.path.exists());
    }

    #[test]
    fn fail_if_exists_publish_cannot_overwrite_target_created_at_rename_boundary() {
        // Production mutation caught: exists-then-rename overwriting a concurrently created target.
        let fixture = FixtureDir::new();
        let output_dir = fixture.0.join("合并视频");
        fs::create_dir(&output_dir).unwrap();
        let temp_output = fixture.0.join("validated-temp.mp4");
        fs::write(&temp_output, b"new-output").unwrap();
        let final_output = output_dir.join("全集.mp4");

        let error = publish_output_with_hook(
            &fixture.0,
            &temp_output,
            "全集.mp4",
            MergeConflictPolicy::FailIfExists,
            || fs::write(&final_output, b"racing-output").unwrap(),
        )
        .unwrap_err();

        assert_eq!(error.code, "MERGE_OUTPUT_EXISTS");
        assert_eq!(fs::read(&final_output).unwrap(), b"racing-output");
        assert_eq!(fs::read(&temp_output).unwrap(), b"new-output");
    }

    #[test]
    fn output_directory_swapped_to_external_symlink_cannot_redirect_overwrite_publish() {
        // Production mutation caught: resolving the destination path again after directory validation.
        use std::os::unix::fs::symlink;
        let fixture = FixtureDir::new();
        let outside = FixtureDir::new();
        let output_dir = fixture.0.join("合并视频");
        let held_dir = fixture.0.join("held-output-dir");
        fs::create_dir(&output_dir).unwrap();
        let outside_target = outside.0.join("全集.mp4");
        fs::write(&outside_target, b"outside-sentinel").unwrap();
        let temp_output = fixture.0.join("validated-temp.mp4");
        fs::write(&temp_output, b"new-output").unwrap();

        let error = publish_output_with_hook(
            &fixture.0,
            &temp_output,
            "全集.mp4",
            MergeConflictPolicy::Overwrite,
            || {
                fs::rename(&output_dir, &held_dir).unwrap();
                symlink(&outside.0, &output_dir).unwrap();
            },
        )
        .unwrap_err();

        assert_eq!(error.code, "MERGE_OUTPUT_CHANGED");
        assert_eq!(fs::read(&outside_target).unwrap(), b"outside-sentinel");
        assert!(!held_dir.join("全集.mp4").exists());
    }

    #[test]
    fn series_root_swapped_after_validation_cannot_redirect_final_publish() {
        // Production mutation caught: canonicalizing/opening series_root again only at publish time.
        let fixture = FixtureDir::new();
        let outside = FixtureDir::new();
        let canonical_root = fs::canonicalize(&fixture.0).unwrap();
        let canonical_outside = fs::canonicalize(&outside.0).unwrap();
        let held_root = canonical_root.with_extension("held-root");
        let ffprobe = format!(
            r#"#!/bin/sh
last=""
for arg in "$@"; do last="$arg"; done
case "$last" in
  *merged.mp4)
    relative="${{last#'{root}/'}}"
    /bin/mkdir -p "{outside}/$(/usr/bin/dirname "$relative")"
    /bin/cp "$last" "{outside}/$relative"
    /bin/mv "{root}" "{held}"
    /bin/ln -s "{outside}" "{root}"
    ;;
esac
printf '{{"streams":[{{"codec_type":"video","codec_name":"h264","width":16,"height":16,"r_frame_rate":"1/1","time_base":"1/16384"}}],"format":{{"duration":"1.0"}}}}'
"#,
            root = canonical_root.display(),
            outside = canonical_outside.display(),
            held = held_root.display(),
        );
        let tools = fixture.tools(successful_ffmpeg_script(), &ffprobe);
        let source = fixture.merge_input(1, "one.mp4");

        let result = run_merge(
            &tools,
            merge_request(&fixture, vec![source]),
            &CancellationToken::new(),
            |_| {},
        );
        let external_output = canonical_outside.join("合并视频").join("全集.mp4");
        let external_output_exists = external_output.exists();
        let held_temp_exists = fs::read_dir(&held_root)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".hongguo-merge-")
            });
        fs::remove_file(&canonical_root).unwrap();
        fs::rename(&held_root, &canonical_root).unwrap();

        let error = result.unwrap_err();
        assert_eq!(error.code, "MERGE_OUTPUT_CHANGED");
        assert!(!external_output_exists);
        assert!(!external_output.exists());
        assert!(!held_temp_exists);
    }

    #[test]
    fn series_root_swap_then_restore_during_ffmpeg_cannot_redirect_private_temp() {
        // Production mutation caught: deriving FFmpeg temp argv from the replaceable public root.
        let series = FixtureDir::new();
        let tools_fixture = FixtureDir::new();
        let outside = FixtureDir::new();
        let canonical_root = fs::canonicalize(&series.0).unwrap();
        let held_root = canonical_root.with_extension("held-during-ffmpeg");
        let attacker_root = outside.0.join("attacker-root");
        let ffmpeg = format!(
            r#"#!/bin/sh
last=""
for arg in "$@"; do last="$arg"; done
/bin/mv "{root}" "{held}"
/bin/mkdir "{root}"
/bin/mkdir -p "$(/usr/bin/dirname "$last")"
printf 'fixture-output' > "$last"
/bin/mv "{root}" "{attacker}"
/bin/mv "{held}" "{root}"
"#,
            root = canonical_root.display(),
            held = held_root.display(),
            attacker = attacker_root.display(),
        );
        let tools = tools_fixture.tools(&ffmpeg, &probe_script("1.0"));
        let source = series.merge_input(1, "one.mp4");
        let source_bytes = fs::read(&source.path).unwrap();

        let result = run_merge(
            &tools,
            merge_request(&series, vec![source.clone()]),
            &CancellationToken::new(),
            |_| {},
        );
        let attacker_has_output = fs::read_dir(&attacker_root)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.path().join("merged.mp4").exists());

        assert!(
            !attacker_has_output,
            "FFmpeg followed a temp path rooted in the swapped public series directory"
        );
        let result = result.unwrap();
        assert_eq!(fs::read(&result.output_path).unwrap(), b"fixture-output");
        assert_eq!(fs::read(&source.path).unwrap(), source_bytes);
    }

    #[test]
    fn ffmpeg_temp_paths_are_created_under_held_series_root() {
        // Production mutation caught: creating temp output in the system temp device and later hitting EXDEV.
        let series = FixtureDir::new();
        let tools_fixture = FixtureDir::new();
        let path_log = tools_fixture.0.join("ffmpeg-paths.log");
        let ffmpeg = format!(
            r#"#!/bin/sh
concat=""
last=""
previous=""
for arg in "$@"; do
  if [ "$previous" = "-i" ]; then concat="$arg"; fi
  last="$arg"
  previous="$arg"
done
        /bin/pwd > "{log}"
        printf '%s\n%s\n' "$concat" "$last" >> "{log}"
printf 'fixture-output' > "$last"
printf 'out_time_us=1000000\nprogress=end\n'
"#,
            log = path_log.display(),
        );
        let tools = tools_fixture.tools(&ffmpeg, &probe_script("1.0"));
        let source = series.merge_input(1, "one.mp4");

        run_merge(
            &tools,
            merge_request(&series, vec![source]),
            &CancellationToken::new(),
            |_| {},
        )
        .unwrap();

        let logged = fs::read_to_string(&path_log).unwrap();
        let mut logged_lines = logged.lines();
        let working_directory = PathBuf::from(logged_lines.next().unwrap());
        let concat_arg = logged_lines.next().unwrap();
        let output_arg = logged_lines.next().unwrap();
        assert_eq!(concat_arg, "concat.txt");
        assert_eq!(output_arg, "merged.mp4");
        assert!(working_directory
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".hongguo-merge-"));
        assert_eq!(
            fs::canonicalize(working_directory.parent().unwrap()).unwrap(),
            fs::canonicalize(&series.0).unwrap()
        );
    }

    #[test]
    fn replaced_private_temp_is_rejected_before_output_ffprobe() {
        // Production mutation caught: probing through a replaced private temp path.
        let series = FixtureDir::new();
        let tools_fixture = FixtureDir::new();
        let outside = FixtureDir::new();
        let temp_paths_log = outside.0.join("temp-paths.log");
        let output_probe_marker = outside.0.join("output-probe-called");
        let ffmpeg = format!(
            r#"#!/bin/sh
last=""
for arg in "$@"; do last="$arg"; done
case "$last" in
  */*) temp="$(/usr/bin/dirname "$last")" ;;
  *) temp="$(/bin/pwd)" ;;
esac
held="$temp.held"
/bin/mv "$temp" "$held"
/bin/mkdir "$temp"
printf 'attacker-output' > "$last"
printf '%s\n%s\n' "$temp" "$held" > "{log}"
"#,
            log = temp_paths_log.display(),
        );
        let ffprobe = format!(
            r#"#!/bin/sh
last=""
for arg in "$@"; do last="$arg"; done
case "$last" in
  *one.mp4) ;;
  *) printf 'called' > "{marker}" ;;
esac
printf '{{"streams":[{{"codec_type":"video","codec_name":"h264","width":16,"height":16,"r_frame_rate":"1/1","time_base":"1/16384"}}],"format":{{"duration":"1.0"}}}}'
"#,
            marker = output_probe_marker.display(),
        );
        let tools = tools_fixture.tools(&ffmpeg, &ffprobe);
        let source = series.merge_input(1, "one.mp4");
        let source_bytes = fs::read(&source.path).unwrap();

        let error = run_merge(
            &tools,
            merge_request(&series, vec![source.clone()]),
            &CancellationToken::new(),
            |_| {},
        )
        .unwrap_err();
        let temp_paths = fs::read_to_string(&temp_paths_log).unwrap();
        let mut temp_paths = temp_paths.lines().map(PathBuf::from);
        let attacker_temp = temp_paths.next().unwrap();
        let held_temp = temp_paths.next().unwrap();
        let attacker_output_exists = attacker_temp.join("merged.mp4").exists();
        let held_output_exists = held_temp.join("merged.mp4").exists();
        let _ = fs::remove_dir_all(&attacker_temp);
        let _ = fs::remove_dir_all(&held_temp);

        assert_eq!(error.code, "MERGE_TEMP_CHANGED");
        assert!(!output_probe_marker.exists());
        assert!(!attacker_output_exists);
        assert!(!held_output_exists);
        assert_eq!(fs::read(&source.path).unwrap(), source_bytes);
        assert!(!series.0.join("合并视频").join("全集.mp4").exists());
    }

    #[test]
    fn symlinked_merge_directory_outside_series_is_rejected() {
        // Production mutation caught: trusting a lexical output prefix through a directory symlink.
        use std::os::unix::fs::symlink;
        let fixture = FixtureDir::new();
        let outside = FixtureDir::new();
        symlink(&outside.0, fixture.0.join("合并视频")).unwrap();

        let error = validated_output_path(&fixture.0, "全集.mp4").unwrap_err();

        assert_eq!(error.code, "MERGE_OUTPUT_INVALID");
    }

    #[test]
    #[ignore = "requires explicitly supplied HONGGUO_TEST_FFMPEG/HONGGUO_TEST_FFPROBE; not release proof"]
    fn explicit_local_tools_merge_generated_copyright_free_media() {
        // Production mutation caught: real ffprobe/concat argv diverging from the verified fake boundary.
        let ffmpeg = std::env::var_os("HONGGUO_TEST_FFMPEG")
            .map(PathBuf::from)
            .expect("set HONGGUO_TEST_FFMPEG explicitly");
        let ffprobe = std::env::var_os("HONGGUO_TEST_FFPROBE")
            .map(PathBuf::from)
            .expect("set HONGGUO_TEST_FFPROBE explicitly");
        let tools = MediaTools::from_test_paths(ffmpeg.clone(), ffprobe);
        let fixture = FixtureDir::new();
        let mut inputs = Vec::new();
        for (index, color) in [(1, "blue"), (2, "green")] {
            let path = fixture.0.join(format!("{index}.mp4"));
            let status = std::process::Command::new(&ffmpeg)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                ])
                .arg(format!("color=c={color}:s=16x16:d=1:r=1"))
                .args([
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=1",
                    "-shortest",
                ])
                .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
            let metadata = fs::metadata(&path).unwrap();
            inputs.push(MergeInput {
                episode_index: index,
                path,
                size: metadata.len(),
                modified_unix_nanos: metadata
                    .modified()
                    .unwrap()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            });
        }

        let result = run_merge(
            &tools,
            merge_request(&fixture, inputs),
            &CancellationToken::new(),
            |_| {},
        )
        .unwrap();

        let output_probe = probe_media(&tools, &result.output_path).unwrap();
        assert!((output_probe.duration_seconds - 2.0).abs() <= 1.0);
        assert!(output_probe.audio.is_some());
    }
}
