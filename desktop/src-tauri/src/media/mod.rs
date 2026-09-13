pub mod ai;
mod audio_prepare;
pub mod components;
pub mod deletion;
pub mod hardware;
#[cfg(unix)]
pub mod merge;
#[cfg(windows)]
#[path = "merge_windows.rs"]
pub mod merge;
pub mod model;
#[cfg(unix)]
pub mod process_control;
#[cfg(windows)]
#[path = "process_control_windows.rs"]
pub mod process_control;
pub mod scheduling;
pub mod storage;
pub mod tools;
#[cfg(all(test, unix))]
#[path = "merge_windows.rs"]
mod windows_merge_tests;
#[cfg(windows)]
mod worker_patch;

pub use ai::{select_subtitle_source, NativeAIExecutor, SubtitleSource};

pub use components::{
    ComponentManager, ComponentManifest, ComponentProgress, ComponentRelease, ComponentStatus,
    InstalledComponent,
};
pub use merge::{
    can_stream_copy, probe_media, run_merge, MediaProbe, MergeProgress, MergeResult,
    StreamSignature,
};
pub use model::{
    InputSnapshot, MediaJob, MediaJobKind, MediaJobOutput, MediaJobOutputKind, MediaJobPauseOrigin,
    MediaJobRequest, MediaJobScope, MediaJobStatus, MediaJobTransition, MediaJobsSnapshot,
    MergeConflictPolicy, MergeInput, MergeRequest, StartAIJobRequest, StartMergeInput,
    StartMergeRequest, ValidatedAIJobRequest, ValidatedMergeRequest,
};
pub use process_control::{CancellationToken, ProcessControl};
pub use storage::MediaJobManager;
pub use tools::MediaTools;

use crate::AppError;
use std::{
    collections::{HashMap, HashSet},
    fs,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, UNIX_EPOCH},
};

pub fn validate_merge_request(
    request: &StartMergeRequest,
) -> Result<ValidatedMergeRequest, AppError> {
    if request.book_id.trim().is_empty() || request.title.trim().is_empty() {
        return Err(AppError::new(
            "MEDIA_MERGE_REQUEST_INVALID",
            "合并任务缺少剧目标识或标题",
        ));
    }
    if request.inputs.is_empty() {
        return Err(AppError::new("MERGE_INPUT_INVALID", "合并输入不得为空"));
    }

    let canonical_root = fs::canonicalize(&request.series_root).map_err(output_invalid)?;
    let root_metadata = fs::metadata(&canonical_root).map_err(output_invalid)?;
    if !root_metadata.is_dir() {
        return Err(output_invalid("series root is not a directory"));
    }
    validate_output_component(&request.output_file_name)?;
    validate_destination(&canonical_root, &request.output_file_name)?;

    let mut episode_indices = HashSet::with_capacity(request.inputs.len());
    let mut inputs = Vec::with_capacity(request.inputs.len());
    for input in &request.inputs {
        if input.episode_index == 0 || !episode_indices.insert(input.episode_index) {
            return Err(AppError::new(
                "MERGE_INPUT_INVALID",
                "合并输入集数必须为不重复的正整数",
            ));
        }
        let canonical_path = fs::canonicalize(&input.path).map_err(input_changed)?;
        let metadata = fs::metadata(&canonical_path).map_err(input_changed)?;
        let modified_unix_nanos = metadata_modified_unix_nanos(&metadata)?;
        if !metadata.is_file() {
            return Err(input_changed("input is not a regular file"));
        }
        if !path_is_within(&canonical_root, &canonical_path) {
            return Err(AppError::new(
                "MEDIA_INPUT_OUTSIDE_ROOT",
                "合并输入必须位于已选剧目目录内",
            ));
        }
        inputs.push(MergeInput {
            episode_index: input.episode_index,
            path: canonical_path,
            size: metadata.len(),
            modified_unix_nanos,
        });
    }
    inputs.sort_by_key(|input| input.episode_index);
    let dedupe_key = merge_dedupe_key(
        &request.book_id,
        &inputs,
        MediaJobScope::Merged,
        request.transcode_h264,
    );

    let dedupe_key = match request.mode {
        Some(mode) => format!("{dedupe_key}-{mode:?}-{:?}", request.quality),
        None => dedupe_key,
    };

    Ok(ValidatedMergeRequest {
        book_id: request.book_id.clone(),
        title: request.title.clone(),
        scope: MediaJobScope::Merged,
        series_root: canonical_root,
        output_file_name: request.output_file_name.clone(),
        inputs,
        transcode_h264: request.transcode_h264,
        mode: request.mode,
        quality: request.quality,
        conflict_policy: request.conflict_policy,
        dedupe_key,
    })
}

pub fn validate_ai_request(
    request: &StartAIJobRequest,
    kind: MediaJobKind,
) -> Result<ValidatedAIJobRequest, AppError> {
    if request.book_id.trim().is_empty() || request.title.trim().is_empty() {
        return Err(AppError::new(
            "AI_REQUEST_INVALID",
            "AI 媒体任务缺少剧目标识或标题",
        ));
    }
    if request.inputs.is_empty() {
        return Err(AppError::new("AI_INPUT_INVALID", "AI 处理输入不得为空"));
    }
    if request.scope == MediaJobScope::Merged && request.inputs.len() != 1 {
        return Err(AppError::new(
            "MEDIA_SCOPE_INVALID",
            "合并视频范围必须且只能选择一个输入",
        ));
    }
    let supported_model = match kind {
        MediaJobKind::SeparateBackgroundMusic => {
            matches!(request.model.as_str(), "htdemucs" | "htdemucs_ft")
        }
        MediaJobKind::ExtractSubtitles => matches!(request.model.as_str(), "small" | "medium"),
        MediaJobKind::Merge => false,
    };
    if !supported_model {
        return Err(AppError::new("AI_MODEL_UNSUPPORTED", "不支持所选 AI 模型"));
    }
    if !matches!(request.device.as_str(), "auto" | "cpu" | "cuda") {
        return Err(AppError::new(
            "AI_DEVICE_UNSUPPORTED",
            "不支持所选 AI 计算设备",
        ));
    }

    let canonical_root = fs::canonicalize(&request.series_root).map_err(input_changed)?;
    if !fs::metadata(&canonical_root)
        .map_err(input_changed)?
        .is_dir()
    {
        return Err(input_changed("series root is not a directory"));
    }
    let mut episode_indices = HashSet::with_capacity(request.inputs.len());
    let mut inputs = Vec::with_capacity(request.inputs.len());
    for input in &request.inputs {
        if input.episode_index == 0 || !episode_indices.insert(input.episode_index) {
            return Err(AppError::new(
                "AI_INPUT_INVALID",
                "AI 处理输入集数必须为不重复的正整数",
            ));
        }
        let path = fs::canonicalize(&input.path).map_err(input_changed)?;
        let metadata = fs::metadata(&path).map_err(input_changed)?;
        if !metadata.is_file() || !path_is_within(&canonical_root, &path) {
            return Err(AppError::new(
                "MEDIA_INPUT_OUTSIDE_ROOT",
                "AI 处理输入必须位于已选剧目目录内",
            ));
        }
        inputs.push(MergeInput {
            episode_index: input.episode_index,
            path,
            size: metadata.len(),
            modified_unix_nanos: metadata_modified_unix_nanos(&metadata)?,
        });
    }
    inputs.sort_by_key(|input| input.episode_index);
    let dedupe_key = ai_dedupe_key(
        &request.book_id,
        &inputs,
        kind,
        request.scope,
        &request.model,
        &request.device,
    );
    Ok(ValidatedAIJobRequest {
        book_id: request.book_id.clone(),
        title: request.title.clone(),
        kind,
        scope: request.scope,
        series_root: canonical_root,
        inputs,
        model: request.model.clone(),
        device: request.device.clone(),
        dedupe_key,
    })
}

fn ai_dedupe_key(
    book_id: &str,
    inputs: &[MergeInput],
    kind: MediaJobKind,
    scope: MediaJobScope,
    model: &str,
    device: &str,
) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for bytes in [
        format!("{kind:?}").into_bytes(),
        format!("{scope:?}").into_bytes(),
        book_id.as_bytes().to_vec(),
        model.as_bytes().to_vec(),
        device.as_bytes().to_vec(),
    ] {
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    for input in inputs {
        for byte in input.episode_index.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        for byte in path_identity_bytes(&input.path) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        for byte in input.modified_unix_nanos.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    format!("ai-{hash:016x}")
}

fn revalidate_merge_request(
    request: &ValidatedMergeRequest,
) -> Result<ValidatedMergeRequest, AppError> {
    let canonical_root = fs::canonicalize(&request.series_root).map_err(output_invalid)?;
    let root_metadata = fs::metadata(&canonical_root).map_err(output_invalid)?;
    if !root_metadata.is_dir() {
        return Err(output_invalid("series root is not a directory"));
    }
    validate_output_component(&request.output_file_name)?;
    validate_destination(&canonical_root, &request.output_file_name)?;

    let mut episode_indices = HashSet::with_capacity(request.inputs.len());
    let mut inputs = Vec::with_capacity(request.inputs.len());
    for input in &request.inputs {
        if input.episode_index == 0 || !episode_indices.insert(input.episode_index) {
            return Err(AppError::new(
                "MERGE_INPUT_INVALID",
                "合并输入集数必须为不重复的正整数",
            ));
        }
        let canonical_path = fs::canonicalize(&input.path).map_err(input_changed)?;
        let metadata = fs::metadata(&canonical_path).map_err(input_changed)?;
        let modified_unix_nanos = metadata_modified_unix_nanos(&metadata)?;
        if !metadata.is_file()
            || metadata.len() != input.size
            || modified_unix_nanos != input.modified_unix_nanos
        {
            return Err(input_changed("input snapshot mismatch"));
        }
        if !path_is_within(&canonical_root, &canonical_path) {
            return Err(AppError::new(
                "MEDIA_INPUT_OUTSIDE_ROOT",
                "合并输入必须位于已选剧目目录内",
            ));
        }
        inputs.push(MergeInput {
            episode_index: input.episode_index,
            path: canonical_path,
            size: metadata.len(),
            modified_unix_nanos,
        });
    }
    inputs.sort_by_key(|input| input.episode_index);
    let dedupe_key = merge_dedupe_key(
        &request.book_id,
        &inputs,
        MediaJobScope::Merged,
        request.transcode_h264,
    );

    let dedupe_key = match request.mode {
        Some(mode) => format!("{dedupe_key}-{mode:?}-{:?}", request.quality),
        None => dedupe_key,
    };

    Ok(ValidatedMergeRequest {
        book_id: request.book_id.clone(),
        title: request.title.clone(),
        scope: MediaJobScope::Merged,
        series_root: canonical_root,
        output_file_name: request.output_file_name.clone(),
        inputs,
        transcode_h264: request.transcode_h264,
        mode: request.mode,
        quality: request.quality,
        conflict_policy: request.conflict_policy,
        dedupe_key,
    })
}

fn revalidate_ai_request(
    request: &ValidatedAIJobRequest,
) -> Result<ValidatedAIJobRequest, AppError> {
    let start = StartAIJobRequest {
        book_id: request.book_id.clone(),
        title: request.title.clone(),
        series_root: request.series_root.clone(),
        scope: request.scope,
        inputs: request
            .inputs
            .iter()
            .map(|input| StartMergeInput {
                episode_index: input.episode_index,
                path: input.path.clone(),
            })
            .collect(),
        model: request.model.clone(),
        device: request.device.clone(),
    };
    let validated = validate_ai_request(&start, request.kind)?;
    if &validated != request {
        return Err(AppError::new(
            "MEDIA_INPUT_CHANGED",
            "AI 处理输入已移动或发生变化",
        ));
    }
    Ok(validated)
}

fn metadata_modified_unix_nanos(metadata: &fs::Metadata) -> Result<u128, AppError> {
    metadata
        .modified()
        .and_then(|modified| {
            modified
                .duration_since(UNIX_EPOCH)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
        })
        .map(|duration| duration.as_nanos())
        .map_err(input_changed)
}

fn validate_output_component(output_file_name: &str) -> Result<(), AppError> {
    let path = Path::new(output_file_name);
    let mut components = path.components();
    if output_file_name.is_empty()
        || output_file_name.contains('\0')
        || output_file_name.contains('\\')
        || path.is_absolute()
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || path.extension().and_then(|extension| extension.to_str()) != Some("mp4")
    {
        return Err(output_invalid("output name is not one safe mp4 component"));
    }
    Ok(())
}

fn validate_destination(root: &Path, output_file_name: &str) -> Result<(), AppError> {
    let output_directory = root.join("合并视频");
    match fs::symlink_metadata(&output_directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(output_invalid(
                "merge output directory is not a real directory",
            ));
        }
        Ok(_) => {
            let canonical_output_directory =
                fs::canonicalize(&output_directory).map_err(output_invalid)?;
            if !path_is_within(root, &canonical_output_directory) {
                return Err(output_invalid("merge output directory escaped series root"));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(output_invalid(error)),
    }
    let destination = output_directory.join(output_file_name);
    if !path_is_within(root, &destination) {
        return Err(output_invalid("merge destination escaped series root"));
    }
    if let Ok(metadata) = fs::symlink_metadata(&destination) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(output_invalid("merge destination is not a regular file"));
        }
    }
    Ok(())
}

fn merge_dedupe_key(
    book_id: &str,
    inputs: &[MergeInput],
    scope: MediaJobScope,
    transcode_h264: bool,
) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    let mut write = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    };
    write(b"merge");
    write(book_id.as_bytes());
    write(match scope {
        MediaJobScope::Episodes => b"episodes",
        MediaJobScope::Merged => b"merged",
    });
    write(if transcode_h264 { b"h264" } else { b"copy" });
    for input in inputs {
        write(&input.episode_index.to_le_bytes());
        write(&path_identity_bytes(&input.path));
        write(&input.size.to_le_bytes());
        write(&input.modified_unix_nanos.to_le_bytes());
    }
    format!("merge-{hash:016x}")
}

#[cfg(unix)]
fn path_identity_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn path_identity_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
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

fn path_is_within(root: &Path, candidate: &Path) -> bool {
    if candidate.starts_with(root) {
        return true;
    }
    let stripped_root = strip_unix_private_prefix(root);
    let stripped_candidate = strip_unix_private_prefix(candidate);
    stripped_candidate.starts_with(&stripped_root)
}

fn strip_unix_private_prefix(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    for prefix in ["/private/tmp", "/private/var", "/private/etc"] {
        if text == prefix {
            return PathBuf::from(&prefix["/private".len()..]);
        }
        let nested = format!("{prefix}/");
        if let Some(rest) = text.strip_prefix(&nested) {
            return PathBuf::from(format!("{}/{rest}", &prefix["/private".len()..]));
        }
    }
    path.to_path_buf()
}

pub trait MergeExecutor: Send + Sync + 'static {
    fn execute_with_budget(
        &self,
        request: MergeRequest,
        cancellation: &CancellationToken,
        progress: &mut dyn FnMut(MergeProgress),
        _budget: scheduling::ExecutionBudget,
    ) -> Result<MergeResult, AppError> {
        self.execute(request, cancellation, progress)
    }
    fn execute(
        &self,
        request: MergeRequest,
        cancellation: &CancellationToken,
        progress: &mut dyn FnMut(MergeProgress),
    ) -> Result<MergeResult, AppError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AIExecutionResult {
    pub output_path: PathBuf,
    pub outputs: Vec<MediaJobOutput>,
}

pub trait AIExecutor: Send + Sync + 'static {
    fn scheduling_device(&self, request: &ValidatedAIJobRequest) -> String {
        request.device.clone()
    }

    fn execute_with_budget(
        &self,
        request: ValidatedAIJobRequest,
        cancellation: &CancellationToken,
        progress: &mut dyn FnMut(MergeProgress),
        _budget: scheduling::ExecutionBudget,
    ) -> Result<AIExecutionResult, AppError> {
        self.execute(request, cancellation, progress)
    }

    fn execute(
        &self,
        request: ValidatedAIJobRequest,
        cancellation: &CancellationToken,
        progress: &mut dyn FnMut(MergeProgress),
    ) -> Result<AIExecutionResult, AppError>;
}

pub(crate) struct UnavailableAIExecutor;

impl AIExecutor for UnavailableAIExecutor {
    fn execute(
        &self,
        _request: ValidatedAIJobRequest,
        _cancellation: &CancellationToken,
        _progress: &mut dyn FnMut(MergeProgress),
    ) -> Result<AIExecutionResult, AppError> {
        Err(AppError::new(
            "AI_COMPONENT_NOT_INSTALLED",
            "AI 运行环境或所选模型尚未安装",
        ))
    }
}

pub trait MediaJobEventSink: Send + Sync + 'static {
    fn emit(&self, job: MediaJob);
}

pub struct NativeMergeExecutor {
    tools: Result<MediaTools, AppError>,
}

impl NativeMergeExecutor {
    pub fn from_packaged_tools() -> Self {
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
        Self { tools }
    }
}

impl MergeExecutor for NativeMergeExecutor {
    #[cfg(windows)]
    fn execute_with_budget(
        &self,
        request: MergeRequest,
        cancellation: &CancellationToken,
        progress: &mut dyn FnMut(MergeProgress),
        budget: scheduling::ExecutionBudget,
    ) -> Result<MergeResult, AppError> {
        let tools = self.tools.as_ref().map_err(Clone::clone)?;
        merge::run_merge_with_budget(tools, request, cancellation, Some(budget), progress)
    }
    fn execute(
        &self,
        request: MergeRequest,
        cancellation: &CancellationToken,
        progress: &mut dyn FnMut(MergeProgress),
    ) -> Result<MergeResult, AppError> {
        let tools = self.tools.as_ref().map_err(Clone::clone)?;
        run_merge(tools, request, cancellation, progress)
    }
}

struct RunningJob {
    token: CancellationToken,
    kind: MediaJobKind,
    budget: scheduling::ExecutionBudget,
}

type RunningJobs = Arc<Mutex<HashMap<String, RunningJob>>>;

pub struct MediaJobService {
    manager: Arc<MediaJobManager>,
    event_sink: Arc<dyn MediaJobEventSink>,
    wake_sender: Option<mpsc::Sender<()>>,
    running: RunningJobs,
    pending_removals: Arc<Mutex<HashSet<String>>>,
    merge_request_lock: Mutex<()>,
    shutting_down: Arc<AtomicBool>,
    concurrency: Arc<AtomicUsize>,
    queue_reason: Arc<Mutex<String>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl MediaJobService {
    pub fn new(
        manager: Arc<MediaJobManager>,
        executor: Arc<dyn MergeExecutor>,
        event_sink: Arc<dyn MediaJobEventSink>,
    ) -> Self {
        Self::new_with_ai(
            manager,
            executor,
            Arc::new(UnavailableAIExecutor),
            event_sink,
        )
    }

    pub fn new_with_ai(
        manager: Arc<MediaJobManager>,
        executor: Arc<dyn MergeExecutor>,
        ai_executor: Arc<dyn AIExecutor>,
        event_sink: Arc<dyn MediaJobEventSink>,
    ) -> Self {
        Self::with_resource_probe(
            manager,
            executor,
            ai_executor,
            event_sink,
            scheduling::native_probe(),
        )
    }

    fn with_resource_probe(
        manager: Arc<MediaJobManager>,
        executor: Arc<dyn MergeExecutor>,
        ai_executor: Arc<dyn AIExecutor>,
        event_sink: Arc<dyn MediaJobEventSink>,
        probe: scheduling::ResourceProbe,
    ) -> Self {
        Self::with_resource_probe_and_limit(manager, executor, ai_executor, event_sink, probe, 0)
    }

    pub fn new_configured(
        manager: Arc<MediaJobManager>,
        executor: Arc<dyn MergeExecutor>,
        ai_executor: Arc<dyn AIExecutor>,
        event_sink: Arc<dyn MediaJobEventSink>,
        concurrency: usize,
    ) -> Self {
        Self::with_resource_probe_and_limit(
            manager,
            executor,
            ai_executor,
            event_sink,
            scheduling::native_probe(),
            concurrency,
        )
    }

    fn with_resource_probe_and_limit(
        manager: Arc<MediaJobManager>,
        executor: Arc<dyn MergeExecutor>,
        ai_executor: Arc<dyn AIExecutor>,
        event_sink: Arc<dyn MediaJobEventSink>,
        probe: scheduling::ResourceProbe,
        initial_concurrency: usize,
    ) -> Self {
        let resume_queued = manager
            .snapshot()
            .jobs
            .iter()
            .any(|job| job.status == MediaJobStatus::Queued);
        let (wake_sender, wake_receiver) = mpsc::channel();
        let running = Arc::new(Mutex::new(HashMap::new()));
        let pending_removals = Arc::new(Mutex::new(HashSet::new()));
        let shutting_down = Arc::new(AtomicBool::new(false));
        let concurrency = Arc::new(AtomicUsize::new(initial_concurrency.min(10)));
        let queue_reason = Arc::new(Mutex::new(String::new()));
        let worker_context = MediaWorkerContext {
            manager: manager.clone(),
            executor,
            ai_executor,
            event_sink: event_sink.clone(),
            running: running.clone(),
            pending_removals: pending_removals.clone(),
            shutting_down: shutting_down.clone(),
            concurrency: concurrency.clone(),
            queue_reason: queue_reason.clone(),
        };
        let completion_sender = wake_sender.clone();
        let worker = thread::Builder::new()
            .name("media-job-scheduler".into())
            .spawn(move || worker_loop(worker_context, wake_receiver, completion_sender, probe))
            .expect("media worker thread should start");
        let service = Self {
            manager,
            event_sink,
            wake_sender: Some(wake_sender),
            running,
            pending_removals,
            merge_request_lock: Mutex::new(()),
            shutting_down,
            concurrency,
            queue_reason,
            worker: Mutex::new(Some(worker)),
        };
        if resume_queued {
            service.wake_worker();
        }
        service
    }

    pub fn set_concurrency(&self, value: usize) {
        self.concurrency
            .store(value.min(10), AtomicOrdering::Release);
        self.wake_worker();
    }

    pub fn scheduling_status(&self) -> serde_json::Value {
        serde_json::json!({
            "windows": cfg!(windows),
            "concurrency": self.concurrency.load(AtomicOrdering::Acquire),
            "reason": self.queue_reason.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        })
    }

    pub fn snapshot(&self) -> MediaJobsSnapshot {
        self.manager.snapshot()
    }

    pub fn mark_notified(&self, job_id: &str, succeeded: bool) -> Result<MediaJob, AppError> {
        let job = self.manager.mark_notified(job_id, succeeded)?;
        safe_emit(&self.event_sink, job.clone());
        Ok(job)
    }

    pub fn is_validated_upload_source(&self, path: &Path) -> bool {
        let Ok(candidate) = fs::canonicalize(path) else {
            return false;
        };
        self.manager.snapshot().jobs.iter().any(|job| {
            if job.status != MediaJobStatus::Completed {
                return false;
            }
            if job.kind == MediaJobKind::Merge {
                return job
                    .output_path
                    .as_ref()
                    .and_then(|value| fs::canonicalize(value).ok())
                    .is_some_and(|value| value == candidate);
            }
            job.ai_request
                .as_ref()
                .is_some_and(|request| request.scope == MediaJobScope::Merged)
                && job.outputs.iter().any(|output| {
                    output.kind == MediaJobOutputKind::NoBackgroundMusicVideo
                        && fs::canonicalize(&output.path).is_ok_and(|value| value == candidate)
                })
        })
    }

    pub fn start_merge(&self, request: StartMergeRequest) -> Result<MediaJob, AppError> {
        let _guard = self.merge_request_lock.lock().map_err(|_| {
            AppError::new("MEDIA_JOB_MANAGER_UNAVAILABLE", "媒体任务管理器暂不可用")
        })?;
        if self.has_merged_video(&request.series_root)? {
            return Err(AppError::new(
                "MERGE_OUTPUT_EXISTS",
                "已存在合并视频，请先移走或删除后再合并",
            ));
        }
        let mut validated = validate_merge_request(&request)?;
        validated.conflict_policy = MergeConflictPolicy::FailIfExists;
        // An already-awake worker may claim this job before wake_worker(). Hold
        // its claim lock until the persisted queued snapshot has been emitted.
        let _publication = self.running.lock().map_err(|_| {
            AppError::new("MEDIA_JOB_MANAGER_UNAVAILABLE", "媒体任务管理器暂不可用")
        })?;
        let job = self.manager.enqueue_merge(validated)?;
        safe_emit(&self.event_sink, job.clone());
        self.wake_worker();
        Ok(job)
    }

    pub fn start_ai(
        &self,
        request: StartAIJobRequest,
        kind: MediaJobKind,
    ) -> Result<MediaJob, AppError> {
        let validated = validate_ai_request(&request, kind)?;
        let _publication = self.running.lock().map_err(|_| {
            AppError::new("MEDIA_JOB_MANAGER_UNAVAILABLE", "媒体任务管理器暂不可用")
        })?;
        let job = self.manager.enqueue_ai(validated)?;
        safe_emit(&self.event_sink, job.clone());
        self.wake_worker();
        Ok(job)
    }

    pub fn cancel(&self, job_id: &str) -> Result<MediaJob, AppError> {
        let token = self
            .running
            .lock()
            .map_err(|_| AppError::new("MEDIA_JOB_MANAGER_UNAVAILABLE", "媒体任务管理器暂不可用"))?
            .get(job_id)
            .map(|job| job.token.clone())
            .ok_or_else(|| {
                AppError::new("MEDIA_JOB_INVALID_TRANSITION", "只能取消正在运行的媒体任务")
            })?;
        token.cancel();
        self.manager.job(job_id)
    }

    pub fn pause(&self, job_id: &str) -> Result<MediaJob, AppError> {
        let running = self.running.lock().map_err(|_| {
            AppError::new("MEDIA_JOB_MANAGER_UNAVAILABLE", "媒体任务管理器暂不可用")
        })?;
        let current = self.manager.job(job_id)?;
        let job = match current.status {
            MediaJobStatus::Queued => self.manager.pause_queued(job_id)?,
            MediaJobStatus::Running => {
                let token = running.get(job_id).map(|job| &job.token).ok_or_else(|| {
                    AppError::new(
                        "MEDIA_JOB_INVALID_TRANSITION",
                        "媒体任务当前状态不允许该操作",
                    )
                })?;
                token.pause()?;
                match self.manager.pause_running(job_id) {
                    Ok(job) => job,
                    Err(error) => {
                        let _ = token.resume();
                        return Err(error);
                    }
                }
            }
            _ => {
                return Err(AppError::new(
                    "MEDIA_JOB_INVALID_TRANSITION",
                    "媒体任务当前状态不允许该操作",
                ))
            }
        };
        drop(running);
        safe_emit(&self.event_sink, job.clone());
        Ok(job)
    }

    pub fn resume(&self, job_id: &str) -> Result<MediaJob, AppError> {
        let running = self.running.lock().map_err(|_| {
            AppError::new("MEDIA_JOB_MANAGER_UNAVAILABLE", "媒体任务管理器暂不可用")
        })?;
        let current = self.manager.job(job_id)?;
        let (job, wake_worker) = match (current.status, current.pause_origin) {
            (MediaJobStatus::Paused, Some(MediaJobPauseOrigin::Queued)) => {
                (self.manager.resume_queued(job_id)?, true)
            }
            (MediaJobStatus::Paused, Some(MediaJobPauseOrigin::Running)) => {
                let token = running.get(job_id).map(|job| &job.token).ok_or_else(|| {
                    AppError::new(
                        "MEDIA_JOB_INVALID_TRANSITION",
                        "媒体任务当前状态不允许该操作",
                    )
                })?;
                token.resume()?;
                match self.manager.resume_running(job_id) {
                    Ok(job) => (job, false),
                    Err(error) => {
                        let _ = token.pause();
                        return Err(error);
                    }
                }
            }
            _ => {
                return Err(AppError::new(
                    "MEDIA_JOB_INVALID_TRANSITION",
                    "媒体任务当前状态不允许该操作",
                ))
            }
        };
        drop(running);
        safe_emit(&self.event_sink, job.clone());
        if wake_worker {
            self.wake_worker();
        }
        Ok(job)
    }

    pub fn delete(&self, job_id: &str) -> Result<(), AppError> {
        let running = self.running.lock().map_err(|_| {
            AppError::new("MEDIA_JOB_MANAGER_UNAVAILABLE", "媒体任务管理器暂不可用")
        })?;
        let current = self.manager.job(job_id)?;
        let is_active_process = current.status == MediaJobStatus::Running
            || (current.status == MediaJobStatus::Paused
                && current.pause_origin == Some(MediaJobPauseOrigin::Running));
        if is_active_process {
            let token = running.get(job_id).map(|job| &job.token).ok_or_else(|| {
                AppError::new(
                    "MEDIA_JOB_INVALID_TRANSITION",
                    "媒体任务当前状态不允许该操作",
                )
            })?;
            if token.is_paused() {
                token.resume()?;
            }
            self.pending_removals
                .lock()
                .map_err(|_| {
                    AppError::new("MEDIA_JOB_MANAGER_UNAVAILABLE", "媒体任务管理器暂不可用")
                })?
                .insert(job_id.to_string());
            token.cancel();
        } else {
            deletion::remove_outputs(&current, &self.manager.snapshot().jobs)?;
            self.manager.remove(job_id)?;
        }
        Ok(())
    }

    pub fn has_merged_video(&self, series_root: &Path) -> Result<bool, AppError> {
        has_merged_video_in_series(series_root)
    }

    pub fn find_merged_video(&self, series_root: &Path) -> Result<Option<PathBuf>, AppError> {
        find_merged_video_in_series(series_root)
    }

    pub fn retry(&self, job_id: &str) -> Result<MediaJob, AppError> {
        self.retry_with_merge_mode(job_id, None)
    }

    fn retry_with_merge_mode(
        &self,
        job_id: &str,
        mode: Option<model::MergeMode>,
    ) -> Result<MediaJob, AppError> {
        // Keep the same lock order as start_merge: request validation, then claim.
        let _request_guard = self.merge_request_lock.lock().map_err(|_| {
            AppError::new("MEDIA_JOB_MANAGER_UNAVAILABLE", "媒体任务管理器暂不可用")
        })?;
        let _publication = self.running.lock().map_err(|_| {
            AppError::new("MEDIA_JOB_MANAGER_UNAVAILABLE", "媒体任务管理器暂不可用")
        })?;
        let original_job = self.manager.job(job_id)?;
        let job = if original_job.kind == MediaJobKind::Merge {
            let mut original = self.manager.merge_request(job_id)?;
            if let Some(mode) = mode {
                original.mode = Some(mode);
            }
            if self.has_merged_video(&original.series_root)? {
                return Err(AppError::new(
                    "MERGE_OUTPUT_EXISTS",
                    "已存在合并视频，请先移走或删除后再合并",
                ));
            }
            let revalidated = revalidate_merge_request(&original)?;
            if mode == Some(model::MergeMode::Auto) {
                self.manager.retry_automation_merge(job_id, revalidated)?
            } else {
                self.manager.retry_merge(job_id, revalidated)?
            }
        } else {
            let original = self.manager.ai_request(job_id)?;
            let revalidated = revalidate_ai_request(&original)?;
            self.manager.retry_ai(job_id, revalidated)?
        };
        safe_emit(&self.event_sink, job.clone());
        self.wake_worker();
        Ok(job)
    }

    pub(crate) fn retry_automation_merge(&self, job_id: &str) -> Result<MediaJob, AppError> {
        self.retry_with_merge_mode(job_id, Some(model::MergeMode::Auto))
    }

    fn wake_worker(&self) {
        if let Some(sender) = &self.wake_sender {
            let _ = sender.send(());
        }
    }
}

impl Drop for MediaJobService {
    fn drop(&mut self) {
        self.shutting_down.store(true, AtomicOrdering::Release);
        if let Ok(running) = self.running.lock() {
            for job in running.values() {
                job.token.cancel();
            }
        }
        self.wake_worker();
        self.wake_sender.take();
        if let Ok(worker) = self.worker.get_mut() {
            if let Some(worker) = worker.take() {
                let _ = worker.join();
            }
        }
    }
}

#[derive(Clone)]
struct MediaWorkerContext {
    manager: Arc<MediaJobManager>,
    executor: Arc<dyn MergeExecutor>,
    ai_executor: Arc<dyn AIExecutor>,
    event_sink: Arc<dyn MediaJobEventSink>,
    running: RunningJobs,
    pending_removals: Arc<Mutex<HashSet<String>>>,
    shutting_down: Arc<AtomicBool>,
    concurrency: Arc<AtomicUsize>,
    queue_reason: Arc<Mutex<String>>,
}

fn reap_finished_workers(
    workers: &mut HashMap<String, JoinHandle<()>>,
    running: &RunningJobs,
    finished_receiver: &mpsc::Receiver<String>,
) {
    // Completion is sent after execute_job returns, before the wakeup.
    // is_finished alone can still be false at that wakeup, losing the
    // notification and leaving the next queued job waiting for the poll.
    let mut finished: HashSet<_> = finished_receiver.try_iter().collect();
    finished.extend(
        workers
            .iter()
            .filter(|(_, worker)| worker.is_finished())
            .map(|(id, _)| id.clone()),
    );
    for id in finished {
        if let Some(worker) = workers.remove(&id) {
            let _ = worker.join();
        }
        running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }
}

fn worker_loop(
    context: MediaWorkerContext,
    wake_receiver: mpsc::Receiver<()>,
    completion_sender: mpsc::Sender<()>,
    mut probe: scheduling::ResourceProbe,
) {
    let mut workers: HashMap<String, JoinHandle<()>> = HashMap::new();
    let (finished_sender, finished_receiver) = mpsc::channel::<String>();
    loop {
        // Reap before admission: paused tasks still occupy their slots, and a
        // terminal event is fully published before that slot can be reused.
        reap_finished_workers(&mut workers, &context.running, &finished_receiver);
        if context.shutting_down.load(AtomicOrdering::Acquire) {
            break;
        }
        #[cfg(windows)]
        rebalance_cpu(&context.running);
        context
            .queue_reason
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        if !context
            .manager
            .snapshot()
            .jobs
            .iter()
            .any(|job| job.status == MediaJobStatus::Queued)
        {
            if wake_receiver.recv_timeout(Duration::from_secs(2))
                == Err(mpsc::RecvTimeoutError::Disconnected)
            {
                break;
            }
            continue;
        }
        let mut resources = probe();
        loop {
            let claimed = {
                let mut running = context.running.lock().unwrap_or_else(|e| e.into_inner());
                if context.shutting_down.load(AtomicOrdering::Acquire) {
                    break;
                }
                if !cfg!(windows)
                    && running
                        .values()
                        .any(|job| !scheduling::is_parallel_kind(job.kind))
                {
                    *context
                        .queue_reason
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = "等待当前合并任务完成".into();
                    break;
                }
                let active: Vec<_> = running.values().map(|job| job.budget).collect();
                let concurrency = if cfg!(windows) {
                    context.concurrency.load(AtomicOrdering::Acquire)
                } else {
                    0
                };
                // Take the oldest runnable job, allowing an available GPU/CPU
                // lane to pass a job waiting on the other device on Windows.
                let mut selected = None;
                for mut candidate in context
                    .manager
                    .snapshot()
                    .jobs
                    .into_iter()
                    .filter(|j| j.status == MediaJobStatus::Queued)
                {
                    if let Some(request) = candidate.ai_request.as_mut() {
                        request.device = context.ai_executor.scheduling_device(request);
                    }
                    match scheduling::admit_configured(&candidate, &active, resources, concurrency)
                    {
                        Ok(budget) => {
                            selected = Some((candidate.id, budget));
                            break;
                        }
                        Err(reason) => {
                            *context
                                .queue_reason
                                .lock()
                                .unwrap_or_else(|e| e.into_inner()) = reason.into();
                            if !cfg!(windows) {
                                break;
                            }
                        }
                    }
                }
                let Some((candidate_id, budget)) = selected else {
                    break;
                };
                match context.manager.claim_queued(&candidate_id) {
                    Ok(Some(job)) if context.shutting_down.load(AtomicOrdering::Acquire) => {
                        let _ = context.manager.restore_claimed_to_queued(&job.id);
                        break;
                    }
                    Ok(Some(job)) => {
                        let token = CancellationToken::new();
                        running.insert(
                            job.id.clone(),
                            RunningJob {
                                token: token.clone(),
                                kind: job.kind,
                                budget,
                            },
                        );
                        (job, token, budget)
                    }
                    _ => break,
                }
            };
            let (job, token, budget) = claimed;
            // The probe is shared by this admission pass. Reserve only NEW launches;
            // its free-memory counters already include older workers' allocations.
            if cfg!(windows) {
                resources.available_memory = resources
                    .available_memory
                    .map(|v| v.saturating_sub(budget.memory));
                if let Some(gpu) = resources.gpu.as_mut() {
                    gpu.free_memory = gpu.free_memory.saturating_sub(budget.gpu_memory);
                }
            }
            #[cfg(windows)]
            rebalance_cpu(&context.running);
            let id = job.id.clone();
            let worker_context = context.clone();
            let notify = completion_sender.clone();
            let finished_notify = finished_sender.clone();
            let finished_id = id.clone();
            // Publish Running before the worker can emit progress.
            safe_emit(&context.event_sink, job.clone());
            match thread::Builder::new()
                .name(format!("media-job-{id}"))
                .spawn(move || {
                    execute_job(worker_context, job, token, budget);
                    let _ = finished_notify.send(finished_id);
                    let _ = notify.send(());
                }) {
                Ok(worker) => {
                    workers.insert(id, worker);
                }
                Err(_) => {
                    if let Ok(failed) = context.manager.update(
                        &id,
                        MediaJobTransition::Fail {
                            code: "MEDIA_WORKER_FAILED".into(),
                            message: "无法启动媒体任务工作线程，可重试".into(),
                        },
                    ) {
                        safe_emit(&context.event_sink, failed);
                    }
                    context
                        .running
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&id);
                }
            }
        }
        if wake_receiver.recv_timeout(Duration::from_secs(2))
            == Err(mpsc::RecvTimeoutError::Disconnected)
        {
            break;
        }
    }
    // Drop waits for every child to stop; queued jobs remain queued for next launch.
    for (_, worker) in workers {
        let _ = worker.join();
    }
    context
        .running
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
}

#[cfg(windows)]
fn rebalance_cpu(running: &RunningJobs) {
    let jobs = running.lock().unwrap_or_else(|e| e.into_inner());
    let active = jobs.values().filter(|j| !j.token.is_paused()).count();
    let rate = scheduling::cpu_share(active);
    for job in jobs.values() {
        // A failure is surfaced when registering a subprocess. Never terminate an
        // otherwise healthy task merely because a driver rejected a live update.
        let _ = job.token.set_cpu_rate(rate);
    }
}

fn execute_job(
    context: MediaWorkerContext,
    job: MediaJob,
    cancellation: CancellationToken,
    budget: scheduling::ExecutionBudget,
) {
    let MediaWorkerContext {
        manager,
        executor,
        ai_executor,
        event_sink,
        pending_removals,
        ..
    } = context;
    if job.merge_request.is_none() && job.ai_request.is_none() {
        if let Ok(failed) = manager.update(
            &job.id,
            MediaJobTransition::Fail {
                code: "MEDIA_JOB_RETRY_UNAVAILABLE".into(),
                message: "媒体任务缺少已验证的原始请求".into(),
            },
        ) {
            safe_emit(&event_sink, failed);
            return;
        }
        return;
    }
    let progress_failure = Arc::new(Mutex::new(None::<AppError>));
    let progress_manager = manager.clone();
    let progress_sink = event_sink.clone();
    let progress_job_id = job.id.clone();
    let progress_cancellation = cancellation.clone();
    let progress_failure_slot = progress_failure.clone();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut update_progress = |progress: MergeProgress| {
            if progress.terminal {
                return;
            }
            if pending_removals
                .lock()
                .map(|pending| pending.contains(&progress_job_id))
                .unwrap_or(true)
            {
                return;
            }
            match progress_manager.update_progress(
                &progress_job_id,
                progress.stage,
                progress.percent,
            ) {
                Ok(updated) => safe_emit(&progress_sink, updated),
                Err(error) => {
                    if let Ok(mut slot) = progress_failure_slot.lock() {
                        if slot.is_none() {
                            *slot = Some(error);
                        }
                    }
                    progress_cancellation.cancel();
                }
            }
        };
        if let Some(request) = job.merge_request.clone() {
            executor
                .execute_with_budget(
                    request.execution_request(),
                    &cancellation,
                    &mut update_progress,
                    budget,
                )
                .map(|result| AIExecutionResult {
                    output_path: result.output_path,
                    outputs: Vec::new(),
                })
        } else if let Some(request) = job.ai_request.clone() {
            ai_executor.execute_with_budget(request, &cancellation, &mut update_progress, budget)
        } else {
            Err(AppError::new(
                "MEDIA_JOB_RETRY_UNAVAILABLE",
                "媒体任务缺少已验证的原始请求",
            ))
        }
    }));

    let _completion = context.running.lock().unwrap_or_else(|e| e.into_inner());
    let remove_after_stop = pending_removals
        .lock()
        .map(|pending| pending.contains(&job.id))
        .unwrap_or(false);
    if remove_after_stop {
        let mut cleanup_job = manager.job(&job.id).unwrap_or_else(|_| job.clone());
        if let Ok(Ok(ref completed)) = result {
            cleanup_job.output_path = Some(completed.output_path.clone());
            cleanup_job.outputs = completed.outputs.clone();
        }
        match deletion::remove_outputs(&cleanup_job, &manager.snapshot().jobs) {
            Ok(()) => {
                let _ = manager.remove(&job.id);
            }
            Err(error) => {
                if let Ok(terminal) = manager.update(
                    &job.id,
                    MediaJobTransition::DeletionFailed {
                        code: error.code,
                        message: error.message,
                        output_path: cleanup_job.output_path,
                        outputs: cleanup_job.outputs,
                    },
                ) {
                    safe_emit(&event_sink, terminal);
                }
            }
        }
        if let Ok(mut pending) = pending_removals.lock() {
            pending.remove(&job.id);
        }
        return;
    }

    let progress_error = progress_failure
        .lock()
        .ok()
        .and_then(|mut failure| failure.take());
    let transition = if let Some(error) = progress_error {
        MediaJobTransition::Fail {
            code: error.code,
            message: error.message,
        }
    } else {
        match result {
            Ok(Ok(result)) => MediaJobTransition::Complete {
                output_path: result.output_path,
                outputs: result.outputs,
            },
            Ok(Err(error)) if matches!(error.code.as_str(), "MERGE_CANCELLED" | "AI_CANCELLED") => {
                MediaJobTransition::Cancel
            }
            Ok(Err(error)) => MediaJobTransition::Fail {
                code: error.code,
                message: error.message,
            },
            Err(_) => MediaJobTransition::Fail {
                code: "MEDIA_WORKER_FAILED".into(),
                message: "媒体任务工作线程异常，可重试该任务".into(),
            },
        }
    };
    if let Ok(terminal) = manager.update(&job.id, transition) {
        safe_emit(&event_sink, terminal);
    }
}

fn has_merged_video_in_series(series_root: &Path) -> Result<bool, AppError> {
    Ok(find_merged_video_in_series(series_root)?.is_some())
}

fn find_merged_video_in_series(series_root: &Path) -> Result<Option<PathBuf>, AppError> {
    let Ok(canonical_root) = fs::canonicalize(series_root) else {
        return Ok(None);
    };
    let Ok(root_metadata) = fs::metadata(&canonical_root) else {
        return Ok(None);
    };
    if !root_metadata.is_dir() {
        return Ok(None);
    }
    let merged_dir = canonical_root.join("合并视频");
    let metadata = match fs::symlink_metadata(&merged_dir) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(None),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(None);
    }
    let Ok(entries) = fs::read_dir(&merged_dir) else {
        return Ok(None);
    };
    let mut found: Option<PathBuf> = None;
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
        {
            continue;
        }
        let Ok(canonical) = fs::canonicalize(&path) else {
            continue;
        };
        if !path_is_within(&canonical_root, &canonical) {
            continue;
        }
        match &found {
            Some(existing) if canonical < *existing => found = Some(canonical),
            None => found = Some(canonical),
            _ => {}
        }
    }
    Ok(found)
}

fn safe_emit(event_sink: &Arc<dyn MediaJobEventSink>, job: MediaJob) {
    let _ = catch_unwind(AssertUnwindSafe(|| event_sink.emit(job)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppError;
    use serde_json::{json, Value};
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicU64, AtomicUsize, Ordering},
            mpsc, Arc, Mutex,
        },
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct FixtureStore {
        root: PathBuf,
        input: PathBuf,
    }

    impl FixtureStore {
        fn new() -> Self {
            let unique = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "hongguo-media-job-test-{}-{}-{unique}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("fixture clock should be after the Unix epoch")
                    .as_nanos()
            ));
            fs::create_dir_all(&root).expect("fixture directory should be created");
            let input = root.join("source.mp4");
            fs::write(&input, b"original-mp4").expect("fixture input should be written");
            Self { root, input }
        }

        fn path(&self) -> &Path {
            &self.root
        }

        fn input_path(&self) -> &Path {
            &self.input
        }

        fn canonical_path(&self) -> PathBuf {
            self.root.join("media-jobs.json")
        }

        fn temporary_path(&self) -> PathBuf {
            self.root.join("media-jobs.json.tmp")
        }

        fn write_jobs(&self, jobs: Vec<Value>) {
            fs::write(
                self.canonical_path(),
                serde_json::to_vec_pretty(&json!({ "version": 1, "jobs": jobs }))
                    .expect("fixture JSON should serialize"),
            )
            .expect("fixture store should be written");
        }
    }

    impl Drop for FixtureStore {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn status_name(status: MediaJobStatus) -> &'static str {
        match status {
            MediaJobStatus::Queued => "queued",
            MediaJobStatus::Running => "running",
            MediaJobStatus::Paused => "paused",
            MediaJobStatus::Completed => "completed",
            MediaJobStatus::Failed => "failed",
            MediaJobStatus::Cancelled => "cancelled",
            MediaJobStatus::Interrupted => "interrupted",
        }
    }

    fn fixture_job(store: &FixtureStore, id: &str, status: MediaJobStatus) -> Value {
        let terminal_fields = match status {
            MediaJobStatus::Completed => json!({
                "outputPath": store.path().join("merged.mp4"),
                "errorCode": null,
                "errorMessage": null
            }),
            MediaJobStatus::Failed => json!({
                "outputPath": null,
                "errorCode": "FFPROBE_FAILED",
                "errorMessage": "输出验证失败"
            }),
            _ => json!({
                "outputPath": null,
                "errorCode": null,
                "errorMessage": null
            }),
        };
        let mut value = json!({
            "id": id,
            "dedupeKey": format!("book-1:{id}"),
            "kind": "merge",
            "status": status_name(status),
            "stage": status_name(status),
            "percent": if status == MediaJobStatus::Completed { 100.0 } else { 25.0 },
            "inputs": [{
                "path": store.input_path(),
                "sizeBytes": 12
            }]
        });
        value
            .as_object_mut()
            .expect("fixture job should be an object")
            .extend(
                terminal_fields
                    .as_object()
                    .expect("terminal fields should be an object")
                    .clone(),
            );
        value
    }

    fn fixture_store_with(status: MediaJobStatus) -> FixtureStore {
        let store = FixtureStore::new();
        store.write_jobs(vec![fixture_job(&store, "job-1", status)]);
        store
    }

    fn merge_request(book: &str, snapshot: &str) -> MediaJobRequest {
        MediaJobRequest {
            dedupe_key: format!("{book}:{snapshot}"),
            kind: MediaJobKind::Merge,
            inputs: vec![InputSnapshot {
                path: PathBuf::from(format!("/downloads/{snapshot}.mp4")),
                size_bytes: 100,
            }],
        }
    }

    #[test]
    fn state_machine_allows_only_declared_edges() {
        let store = FixtureStore::new();
        let manager = MediaJobManager::load(store.path()).unwrap();
        let job = manager
            .enqueue(merge_request("book-1", "snapshot-1"))
            .unwrap();

        let invalid_cancel = manager.cancel(&job.id).unwrap_err();
        assert_eq!(invalid_cancel.code, "MEDIA_JOB_INVALID_TRANSITION");

        manager.update(&job.id, MediaJobTransition::Start).unwrap();
        manager
            .update(
                &job.id,
                MediaJobTransition::Fail {
                    code: "FFMPEG_FAILED".into(),
                    message: "合并失败".into(),
                },
            )
            .unwrap();
        manager.retry(&job.id).unwrap();
        manager.update(&job.id, MediaJobTransition::Start).unwrap();
        manager.cancel(&job.id).unwrap();
        manager.retry(&job.id).unwrap();
        manager.update(&job.id, MediaJobTransition::Start).unwrap();
        manager
            .update(
                &job.id,
                MediaJobTransition::Complete {
                    output_path: store.path().join("merged.mp4"),
                    outputs: Vec::new(),
                },
            )
            .unwrap();

        let invalid_restart = manager
            .update(&job.id, MediaJobTransition::Start)
            .unwrap_err();
        assert_eq!(invalid_restart.code, "MEDIA_JOB_INVALID_TRANSITION");
        assert_eq!(manager.snapshot().jobs[0].status, MediaJobStatus::Completed);
    }

    #[test]
    fn duplicate_active_job_key_is_rejected() {
        let store = FixtureStore::new();
        let manager = MediaJobManager::load(store.path()).unwrap();
        manager
            .enqueue(merge_request("book-1", "snapshot-1"))
            .unwrap();
        let error = manager
            .enqueue(merge_request("book-1", "snapshot-1"))
            .unwrap_err();
        assert_eq!(error.code, "MEDIA_JOB_ALREADY_ACTIVE");
    }

    #[test]
    fn retry_is_rejected_when_another_job_with_the_same_key_is_active() {
        let store = FixtureStore::new();
        let mut failed_job = fixture_job(&store, "failed-job", MediaJobStatus::Failed);
        failed_job["dedupeKey"] = json!("book-1:snapshot-1");
        store.write_jobs(vec![failed_job]);
        let manager = MediaJobManager::load(store.path()).unwrap();
        manager
            .enqueue(merge_request("book-1", "snapshot-1"))
            .unwrap();

        let error = manager.retry("failed-job").unwrap_err();

        assert_eq!(error.code, "MEDIA_JOB_ALREADY_ACTIVE");
        let active_jobs = manager
            .snapshot()
            .jobs
            .into_iter()
            .filter(|job| {
                job.dedupe_key == "book-1:snapshot-1"
                    && matches!(job.status, MediaJobStatus::Queued | MediaJobStatus::Running)
            })
            .count();
        assert_eq!(active_jobs, 1);
    }

    #[test]
    fn running_jobs_restore_as_interrupted_without_touching_inputs() {
        // Production mutation caught: restarting a persisted running job or exposing a stale running stage.
        let store = fixture_store_with(MediaJobStatus::Running);
        let manager = MediaJobManager::load(store.path()).unwrap();
        let job = &manager.snapshot().jobs[0];
        assert_eq!(job.status, MediaJobStatus::Interrupted);
        assert_eq!(job.stage, "interrupted");
        assert!(store.input_path().exists());

        let persisted: Value = serde_json::from_slice(
            &fs::read(store.canonical_path()).expect("recovered store should be readable"),
        )
        .expect("recovered store should contain valid JSON");
        assert_eq!(persisted["jobs"][0]["status"], "interrupted");
    }

    #[test]
    fn completed_outputs_and_failed_diagnostics_survive_restart() {
        let store = FixtureStore::new();
        store.write_jobs(vec![
            fixture_job(&store, "completed-job", MediaJobStatus::Completed),
            fixture_job(&store, "failed-job", MediaJobStatus::Failed),
        ]);

        let jobs = MediaJobManager::load(store.path()).unwrap().snapshot().jobs;

        assert_eq!(jobs[0].status, MediaJobStatus::Completed);
        assert_eq!(jobs[0].output_path, Some(store.path().join("merged.mp4")));
        assert_eq!(jobs[1].status, MediaJobStatus::Failed);
        assert_eq!(jobs[1].error_code.as_deref(), Some("FFPROBE_FAILED"));
        assert_eq!(jobs[1].error_message.as_deref(), Some("输出验证失败"));
    }

    #[test]
    fn corrupt_store_is_quarantined_and_reported_without_failing_load() {
        let store = FixtureStore::new();
        fs::write(store.canonical_path(), b"{not-json").expect("corrupt fixture should be written");

        let snapshot = MediaJobManager::load(store.path()).unwrap().snapshot();

        assert!(snapshot.jobs.is_empty());
        let warning = snapshot
            .warning
            .expect("corruption warning should be exposed");
        assert_eq!(warning.code, "MEDIA_JOBS_CORRUPT_RECOVERED");
        assert!(!store.canonical_path().exists());
        let quarantined = fs::read_dir(store.path())
            .expect("fixture directory should be readable")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .find(|name| name.starts_with("media-jobs.corrupt.") && name.ends_with(".json"))
            .expect("corrupt store should be quarantined");
        assert_eq!(
            fs::read(store.path().join(quarantined)).unwrap(),
            b"{not-json"
        );
    }

    #[test]
    fn enqueue_is_atomically_persisted_to_the_canonical_store() {
        let store = FixtureStore::new();
        let manager = MediaJobManager::load(store.path()).unwrap();

        manager
            .enqueue(merge_request("book-1", "snapshot-1"))
            .unwrap();

        assert!(store.canonical_path().is_file());
        assert!(!store.temporary_path().exists());
        let reloaded = MediaJobManager::load(store.path()).unwrap().snapshot();
        assert_eq!(reloaded.jobs.len(), 1);
        assert_eq!(reloaded.jobs[0].dedupe_key, "book-1:snapshot-1");
    }

    #[test]
    fn cancel_only_changes_running_state_and_never_deletes_inputs() {
        let store = FixtureStore::new();
        let manager = MediaJobManager::load(store.path()).unwrap();
        let job = manager
            .enqueue(MediaJobRequest {
                dedupe_key: "book-1:snapshot-1".into(),
                kind: MediaJobKind::Merge,
                inputs: vec![InputSnapshot {
                    path: store.input_path().to_path_buf(),
                    size_bytes: 12,
                }],
            })
            .unwrap();
        manager.update(&job.id, MediaJobTransition::Start).unwrap();

        manager.cancel(&job.id).unwrap();

        assert_eq!(manager.snapshot().jobs[0].status, MediaJobStatus::Cancelled);
        assert!(store.input_path().exists());
        assert_eq!(fs::read(store.input_path()).unwrap(), b"original-mp4");
    }

    #[test]
    fn public_snapshot_uses_camel_case_fields() {
        let store = FixtureStore::new();
        let manager = MediaJobManager::load(store.path()).unwrap();
        manager
            .enqueue(merge_request("book-1", "snapshot-1"))
            .unwrap();

        let value = serde_json::to_value(manager.snapshot()).unwrap();

        assert_eq!(value["jobs"][0]["dedupeKey"], "book-1:snapshot-1");
        assert_eq!(value["jobs"][0]["outputPath"], Value::Null);
        assert!(value["jobs"][0].get("dedupe_key").is_none());
    }

    #[test]
    fn app_error_serializes_only_stable_public_fields() {
        let error = AppError::with_cause(
            "MEDIA_JOB_STORAGE_IO",
            "媒体任务存储失败",
            "secret/path/media-jobs.json: permission denied",
        );

        assert_eq!(
            serde_json::to_value(error).unwrap(),
            json!({
                "code": "MEDIA_JOB_STORAGE_IO",
                "message": "媒体任务存储失败"
            })
        );
    }

    struct MergeFixture {
        root: PathBuf,
        store: PathBuf,
        series: PathBuf,
    }

    impl MergeFixture {
        fn new() -> Self {
            let unique = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "hongguo-media-controller-test-{}-{}-{unique}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("fixture clock should be after the Unix epoch")
                    .as_nanos()
            ));
            let store = root.join("state");
            let series = root.join("剧目");
            fs::create_dir_all(&store).expect("fixture store should be created");
            fs::create_dir_all(&series).expect("fixture series should be created");
            Self {
                root,
                store,
                series,
            }
        }

        fn write_input(&self, name: &str, bytes: &[u8]) -> StartMergeInput {
            let path = self.series.join(name);
            fs::write(&path, bytes).expect("fixture input should be written");
            StartMergeInput {
                episode_index: 1,
                path,
            }
        }

        fn request(&self, book_id: &str, inputs: Vec<StartMergeInput>) -> StartMergeRequest {
            StartMergeRequest {
                book_id: book_id.into(),
                title: format!("剧目 {book_id}"),
                series_root: self.series.clone(),
                output_file_name: format!("{book_id}.mp4"),
                inputs,
                transcode_h264: true,
                mode: None,
                quality: model::MergeQuality::High,
                conflict_policy: MergeConflictPolicy::FailIfExists,
            }
        }
    }

    impl Drop for MergeFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[derive(Default)]
    struct RecordingSink {
        jobs: Mutex<Vec<MediaJob>>,
    }

    impl MediaJobEventSink for RecordingSink {
        fn emit(&self, job: MediaJob) {
            self.jobs.lock().unwrap().push(job);
        }
    }

    struct PersistCheckingSink {
        manager: Arc<MediaJobManager>,
        jobs: Mutex<Vec<MediaJob>>,
        violations: AtomicUsize,
    }

    impl MediaJobEventSink for PersistCheckingSink {
        fn emit(&self, job: MediaJob) {
            let persisted = self
                .manager
                .snapshot()
                .jobs
                .into_iter()
                .find(|candidate| candidate.id == job.id)
                .expect("emitted job should already be persisted");
            if persisted != job {
                self.violations.fetch_add(1, Ordering::SeqCst);
            }
            self.jobs.lock().unwrap().push(job);
        }
    }

    struct ImmediateExecutor;

    impl MergeExecutor for ImmediateExecutor {
        fn execute(
            &self,
            request: MergeRequest,
            _cancellation: &CancellationToken,
            progress: &mut dyn FnMut(MergeProgress),
        ) -> Result<MergeResult, AppError> {
            progress(MergeProgress {
                stage: "merging".into(),
                percent: 50.0,
                terminal: false,
            });
            Ok(MergeResult {
                output_path: request
                    .series_root
                    .join("合并视频")
                    .join(request.output_file_name),
            })
        }
    }

    fn wait_for_job(service: &MediaJobService, id: &str, status: MediaJobStatus) -> MediaJob {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let job = service
                .snapshot()
                .jobs
                .into_iter()
                .find(|job| job.id == id)
                .expect("job should remain in the snapshot");
            if job.status == status {
                return job;
            }
            assert!(Instant::now() < deadline, "job did not reach {status:?}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn wait_for_terminal_job(service: &MediaJobService, id: &str) -> MediaJob {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let job = service
                .snapshot()
                .jobs
                .into_iter()
                .find(|job| job.id == id)
                .expect("job should remain in the snapshot");
            if matches!(
                job.status,
                MediaJobStatus::Completed | MediaJobStatus::Failed | MediaJobStatus::Cancelled
            ) {
                return job;
            }
            assert!(
                Instant::now() < deadline,
                "job did not reach terminal state"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn start_merge_request_accepts_path_only_inputs_and_validation_builds_snapshot() {
        // Production mutation caught: requiring frontend-provided size or u128 modifiedUnixNanos at the public serde boundary.
        let fixture = MergeFixture::new();
        let path = fixture.series.join("path-only.mp4");
        fs::write(&path, b"path-only").unwrap();
        let metadata = fs::metadata(&path).unwrap();

        let request: StartMergeRequest = serde_json::from_value(json!({
            "bookId": "path-only-book",
            "title": "Path Only",
            "seriesRoot": fixture.series,
            "outputFileName": "path-only.mp4",
            "inputs": [{
                "episodeIndex": 7,
                "path": path
            }],
            "transcodeH264": true,
            "conflictPolicy": "failIfExists"
        }))
        .expect("path-only start payload should deserialize");

        let validated = validate_merge_request(&request).unwrap();

        assert_eq!(validated.inputs.len(), 1);
        assert_eq!(validated.inputs[0].episode_index, 7);
        assert_eq!(validated.inputs[0].path, fs::canonicalize(&path).unwrap());
        assert_eq!(validated.inputs[0].size, metadata.len());
        assert_eq!(
            validated.inputs[0].modified_unix_nanos,
            metadata
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
    }

    #[test]
    fn merge_validation_rejects_empty_zero_and_duplicate_episode_inputs() {
        // Production mutation caught: accepting an empty merge or ambiguous episode ordering.
        let fixture = MergeFixture::new();
        let empty = fixture.request("empty", vec![]);
        assert_eq!(
            validate_merge_request(&empty).unwrap_err().code,
            "MERGE_INPUT_INVALID"
        );

        let mut zero = fixture.write_input("zero.mp4", b"zero");
        zero.episode_index = 0;
        assert_eq!(
            validate_merge_request(&fixture.request("zero", vec![zero]))
                .unwrap_err()
                .code,
            "MERGE_INPUT_INVALID"
        );

        let first = fixture.write_input("one.mp4", b"one");
        let mut duplicate = fixture.write_input("two.mp4", b"two");
        duplicate.episode_index = first.episode_index;
        assert_eq!(
            validate_merge_request(&fixture.request("duplicate", vec![first, duplicate]))
                .unwrap_err()
                .code,
            "MERGE_INPUT_INVALID"
        );
    }

    #[test]
    fn ai_validation_keeps_the_two_operations_independent_and_enforces_scope() {
        let fixture = MergeFixture::new();
        let input = fixture.write_input("one.mp4", b"one");
        let request = StartAIJobRequest {
            book_id: "book-1".into(),
            title: "测试剧".into(),
            series_root: fixture.series.clone(),
            scope: MediaJobScope::Episodes,
            inputs: vec![input.clone()],
            model: "htdemucs".into(),
            device: "auto".into(),
        };

        let separation = validate_ai_request(&request, MediaJobKind::SeparateBackgroundMusic)
            .expect("supported separation request");
        assert_eq!(separation.kind, MediaJobKind::SeparateBackgroundMusic);
        assert!(separation.dedupe_key.starts_with("ai-"));

        let wrong_model =
            validate_ai_request(&request, MediaJobKind::ExtractSubtitles).unwrap_err();
        assert_eq!(wrong_model.code, "AI_MODEL_UNSUPPORTED");

        let mut merged = request;
        merged.scope = MediaJobScope::Merged;
        merged.inputs.push(fixture.write_input("two.mp4", b"two"));
        assert_eq!(
            validate_ai_request(&merged, MediaJobKind::SeparateBackgroundMusic)
                .unwrap_err()
                .code,
            "MEDIA_SCOPE_INVALID"
        );
    }

    #[test]
    fn single_worker_dispatches_ai_jobs_to_the_ai_executor_and_persists_outputs() {
        struct FakeAIExecutor;
        impl AIExecutor for FakeAIExecutor {
            fn execute(
                &self,
                request: ValidatedAIJobRequest,
                _cancellation: &CancellationToken,
                progress: &mut dyn FnMut(MergeProgress),
            ) -> Result<AIExecutionResult, AppError> {
                progress(MergeProgress {
                    stage: "transcribing".into(),
                    percent: 50.0,
                    terminal: false,
                });
                let path = request.series_root.join("字幕/01.srt");
                Ok(AIExecutionResult {
                    output_path: path.clone(),
                    outputs: vec![MediaJobOutput {
                        episode_index: 1,
                        kind: MediaJobOutputKind::Subtitles,
                        path,
                        source: Some("whisperOriginalAudio".into()),
                    }],
                })
            }
        }

        let fixture = MergeFixture::new();
        let input = fixture.write_input("one.mp4", b"one");
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let sink = Arc::new(RecordingSink::default());
        let service = MediaJobService::with_resource_probe(
            manager,
            Arc::new(ImmediateExecutor),
            Arc::new(FakeAIExecutor),
            sink,
            // This tests dispatch/persistence, independent of the host GPU's
            // current workload or the native telemetry warmup interval.
            Box::new(|| scheduling::Resources {
                cores: 16,
                cpu_usage: Some(0.0),
                available_memory: Some(32 * scheduling::GIB),
                gpu: None,
            }),
        );
        let job = service
            .start_ai(
                StartAIJobRequest {
                    book_id: "book-1".into(),
                    title: "测试剧".into(),
                    series_root: fixture.series.clone(),
                    scope: MediaJobScope::Episodes,
                    inputs: vec![input],
                    model: "small".into(),
                    device: "auto".into(),
                },
                MediaJobKind::ExtractSubtitles,
            )
            .unwrap();

        let completed = wait_for_terminal_job(&service, &job.id);
        assert_eq!(completed.status, MediaJobStatus::Completed);
        assert_eq!(completed.outputs.len(), 1);
        assert_eq!(completed.outputs[0].kind, MediaJobOutputKind::Subtitles);
    }

    #[test]
    fn merge_validation_rejects_missing_and_non_file_inputs_without_path_leaks() {
        // Production mutation caught: trusting missing/non-file caller paths or exposing a private source path in errors.
        let fixture = MergeFixture::new();
        let valid = fixture.write_input("private-source.mp4", b"original");

        let mut missing = valid.clone();
        missing.path = fixture.series.join("missing-secret.mp4");
        let missing_error =
            validate_merge_request(&fixture.request("missing", vec![missing])).unwrap_err();
        assert_eq!(missing_error.code, "MEDIA_INPUT_CHANGED");
        assert!(!serde_json::to_string(&missing_error)
            .unwrap()
            .contains("missing-secret"));

        let directory = fixture.series.join("not-a-file");
        fs::create_dir(&directory).unwrap();
        let mut non_file = valid.clone();
        non_file.path = directory;
        let non_file_error =
            validate_merge_request(&fixture.request("non-file", vec![non_file])).unwrap_err();
        assert_eq!(non_file_error.code, "MEDIA_INPUT_CHANGED");
    }

    #[cfg(unix)]
    #[test]
    fn merge_validation_canonicalizes_real_inputs_and_contains_every_path_under_series_root() {
        // Production mutation caught: hashing lexical aliases or allowing an input/symlink destination to escape the selected series.
        use std::os::unix::fs::symlink;

        let fixture = MergeFixture::new();
        let nested = fixture.series.join("nested");
        fs::create_dir(&nested).unwrap();
        let mut aliased = fixture.write_input("canonical.mp4", b"canonical");
        aliased.path = nested.join("..").join("canonical.mp4");
        let validated = validate_merge_request(&fixture.request("canonical", vec![aliased]))
            .expect("lexical alias inside the series should canonicalize");
        assert_eq!(
            validated.inputs[0].path,
            fs::canonicalize(fixture.series.join("canonical.mp4")).unwrap()
        );

        let outside = fixture.root.join("outside.mp4");
        fs::write(&outside, b"outside").unwrap();
        let outside_input = StartMergeInput {
            episode_index: 1,
            path: outside.clone(),
        };
        assert_eq!(
            validate_merge_request(&fixture.request("outside", vec![outside_input]))
                .unwrap_err()
                .code,
            "MEDIA_INPUT_OUTSIDE_ROOT"
        );

        let output_outside = fixture.root.join("escaped-output");
        fs::create_dir(&output_outside).unwrap();
        symlink(&output_outside, fixture.series.join("合并视频")).unwrap();
        let input = fixture.write_input("symlink.mp4", b"symlink");
        assert_eq!(
            validate_merge_request(&fixture.request("symlink", vec![input]))
                .unwrap_err()
                .code,
            "MEDIA_OUTPUT_INVALID"
        );
    }

    #[test]
    fn merge_validation_rejects_invalid_series_roots_and_output_components() {
        // Production mutation caught: accepting a non-directory root, traversal, absolute names, or either separator.
        let fixture = MergeFixture::new();
        let input = fixture.write_input("episode.mp4", b"episode");

        let mut bad_root = fixture.request("root", vec![input.clone()]);
        bad_root.series_root = input.path.clone();
        assert_eq!(
            validate_merge_request(&bad_root).unwrap_err().code,
            "MEDIA_OUTPUT_INVALID"
        );

        for output in [
            "../escape.mp4",
            "/tmp/escape.mp4",
            "nested/x.mp4",
            "nested\\x.mp4",
        ] {
            let mut request = fixture.request("output", vec![input.clone()]);
            request.output_file_name = output.into();
            assert_eq!(
                validate_merge_request(&request).unwrap_err().code,
                "MEDIA_OUTPUT_INVALID",
                "output {output:?} must be rejected"
            );
        }
    }

    #[test]
    fn start_derives_dedupe_from_canonical_identity_and_ignores_display_destination_fields() {
        // Production mutation caught: trusting a caller key or including title/output/conflict fields outside the dedupe contract.
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let sink = Arc::new(RecordingSink::default());
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let service = MediaJobService::new(
            manager,
            Arc::new(BlockingExecutor {
                started: started_tx,
                release: Mutex::new(release_rx),
                active: AtomicUsize::new(0),
                max_active: AtomicUsize::new(0),
            }),
            sink,
        );
        let input = fixture.write_input("dedupe.mp4", b"dedupe");
        let first = fixture.request("book-dedupe", vec![input.clone()]);
        service.start_merge(first).unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let mut duplicate = fixture.request("book-dedupe", vec![input]);
        duplicate.title = "caller changed title".into();
        duplicate.output_file_name = "different.mp4".into();
        duplicate.conflict_policy = MergeConflictPolicy::Overwrite;
        let error = service.start_merge(duplicate).unwrap_err();

        assert_eq!(error.code, "MEDIA_JOB_ALREADY_ACTIVE");
        release_tx.send(()).unwrap();
    }

    #[test]
    fn persisted_merge_request_survives_reload_and_retry_preserves_exact_configuration() {
        // Production mutation caught: persisting only display inputs or rebuilding retry from defaults/current UI state.
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let service = MediaJobService::new(
            manager,
            Arc::new(ErrorExecutor),
            Arc::new(RecordingSink::default()),
        );
        let input = fixture.write_input("retry.mp4", b"retry");
        let mut request = fixture.request("book-retry", vec![input]);
        request.mode = Some(model::MergeMode::Auto);
        request.quality = model::MergeQuality::Compact;
        request.title = "原始标题".into();
        request.output_file_name = "原始输出.mp4".into();
        request.transcode_h264 = false;
        request.conflict_policy = MergeConflictPolicy::Overwrite;
        let job = service.start_merge(request).unwrap();
        wait_for_job(&service, &job.id, MediaJobStatus::Failed);
        drop(service);

        let reloaded = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let expected = reloaded.snapshot().jobs[0]
            .merge_request
            .clone()
            .expect("validated request should be persisted");
        let service = MediaJobService::new(
            reloaded,
            Arc::new(ErrorExecutor),
            Arc::new(RecordingSink::default()),
        );
        let retried = service.retry(&job.id).unwrap();

        assert_eq!(retried.merge_request, Some(expected));
    }

    struct ErrorExecutor;

    #[test]
    fn automation_retry_upgrades_persisted_copy_only_request_without_losing_inputs() {
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let request = fixture.request(
            "auto-fixture-merge",
            vec![fixture.write_input("retry.mp4", b"retry")],
        );
        let expected = validate_merge_request(&request).unwrap();
        let job = manager.enqueue_merge(expected.clone()).unwrap();
        manager.update(&job.id, MediaJobTransition::Start).unwrap();
        manager
            .update(
                &job.id,
                MediaJobTransition::Fail {
                    code: "MERGE_TRANSCODE_REQUIRED".into(),
                    message: "输入媒体参数不一致".into(),
                },
            )
            .unwrap();
        drop(manager);
        let service = MediaJobService::new(
            Arc::new(MediaJobManager::load(&fixture.store).unwrap()),
            Arc::new(ErrorExecutor),
            Arc::new(RecordingSink::default()),
        );
        let retried = service.retry_automation_merge(&job.id).unwrap();
        let actual = retried.merge_request.unwrap();
        assert_eq!(actual.mode, Some(model::MergeMode::Auto));
        assert_eq!(actual.inputs, expected.inputs);
        assert_eq!(actual.output_file_name, expected.output_file_name);
        assert_eq!(actual.quality, expected.quality);
        assert_ne!(actual.dedupe_key, expected.dedupe_key);
    }

    impl MergeExecutor for ErrorExecutor {
        fn execute(
            &self,
            _request: MergeRequest,
            _cancellation: &CancellationToken,
            _progress: &mut dyn FnMut(MergeProgress),
        ) -> Result<MergeResult, AppError> {
            Err(AppError::new("FFMPEG_FAILED", "合并失败"))
        }
    }

    #[test]
    fn retry_revalidates_inputs_and_rejects_another_active_equivalent_job() {
        // Production mutation caught: retrying stale files or bypassing active dedupe protection.
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let input = fixture.write_input("retry-dedupe.mp4", b"retry-dedupe");
        let request = fixture.request("book-retry-dedupe", vec![input.clone()]);
        let validated = validate_merge_request(&request).unwrap();
        let failed = manager.enqueue_merge(validated.clone()).unwrap();
        manager
            .update(&failed.id, MediaJobTransition::Start)
            .unwrap();
        manager
            .update(
                &failed.id,
                MediaJobTransition::Fail {
                    code: "TEST".into(),
                    message: "test".into(),
                },
            )
            .unwrap();
        let active = manager.enqueue_merge(validated).unwrap();
        manager.pause_queued(&active.id).unwrap();

        let service = MediaJobService::new(
            manager.clone(),
            Arc::new(ErrorExecutor),
            Arc::new(RecordingSink::default()),
        );
        assert_eq!(
            service.retry(&failed.id).unwrap_err().code,
            "MEDIA_JOB_ALREADY_ACTIVE"
        );

        manager.resume_queued(&active.id).unwrap();
        manager
            .update(&active.id, MediaJobTransition::Start)
            .unwrap();
        manager
            .update(
                &active.id,
                MediaJobTransition::Fail {
                    code: "TEST".into(),
                    message: "test".into(),
                },
            )
            .unwrap();
        fs::write(&input.path, b"changed").unwrap();
        assert_eq!(
            service.retry(&failed.id).unwrap_err().code,
            "MEDIA_INPUT_CHANGED"
        );
    }

    struct BlockingExecutor {
        started: mpsc::Sender<String>,
        release: Mutex<mpsc::Receiver<()>>,
        active: AtomicUsize,
        max_active: AtomicUsize,
    }

    impl MergeExecutor for BlockingExecutor {
        fn execute(
            &self,
            request: MergeRequest,
            _cancellation: &CancellationToken,
            _progress: &mut dyn FnMut(MergeProgress),
        ) -> Result<MergeResult, AppError> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            self.started.send(request.output_file_name.clone()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(MergeResult {
                output_path: request
                    .series_root
                    .join("合并视频")
                    .join(request.output_file_name),
            })
        }
    }

    struct ShutdownExecutor {
        started: mpsc::Sender<String>,
    }

    impl MergeExecutor for ShutdownExecutor {
        fn execute(
            &self,
            request: MergeRequest,
            cancellation: &CancellationToken,
            _progress: &mut dyn FnMut(MergeProgress),
        ) -> Result<MergeResult, AppError> {
            self.started.send(request.output_file_name.clone()).unwrap();
            if request.output_file_name == "running.mp4" {
                while !cancellation.is_cancelled() {
                    thread::sleep(Duration::from_millis(5));
                }
                return Err(AppError::new("MERGE_CANCELLED", "合并已取消"));
            }
            Ok(MergeResult {
                output_path: request
                    .series_root
                    .join("合并视频")
                    .join(request.output_file_name),
            })
        }
    }

    #[test]
    fn service_drop_cancels_running_job_without_claiming_later_queued_backlog() {
        // Production mutation caught: draining queued backlog inside the worker after shutdown has started.
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let (started_tx, started_rx) = mpsc::channel();
        let service = MediaJobService::new(
            manager.clone(),
            Arc::new(ShutdownExecutor {
                started: started_tx,
            }),
            Arc::new(RecordingSink::default()),
        );
        let mut running_request = fixture.request(
            "shutdown-running",
            vec![fixture.write_input("running-input.mp4", b"a")],
        );
        running_request.output_file_name = "running.mp4".into();
        let running = service.start_merge(running_request).unwrap();
        let mut queued_request = fixture.request(
            "shutdown-queued",
            vec![fixture.write_input("queued-input.mp4", b"b")],
        );
        queued_request.output_file_name = "queued.mp4".into();
        let queued = service.start_merge(queued_request).unwrap();
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "running.mp4"
        );

        drop(service);

        let starts = started_rx.try_iter().collect::<Vec<_>>();
        let snapshot = manager.snapshot();
        let running_after_drop = snapshot
            .jobs
            .iter()
            .find(|job| job.id == running.id)
            .unwrap();
        let queued_after_drop = snapshot
            .jobs
            .iter()
            .find(|job| job.id == queued.id)
            .unwrap();
        assert!(starts.is_empty(), "queued backlog was executed: {starts:?}");
        assert_eq!(running_after_drop.status, MediaJobStatus::Cancelled);
        assert_eq!(queued_after_drop.status, MediaJobStatus::Queued);
    }

    struct HoldingAIExecutor {
        started: mpsc::Sender<String>,
    }

    impl AIExecutor for HoldingAIExecutor {
        fn execute(
            &self,
            request: ValidatedAIJobRequest,
            cancellation: &CancellationToken,
            _progress: &mut dyn FnMut(MergeProgress),
        ) -> Result<AIExecutionResult, AppError> {
            self.started.send(request.book_id).unwrap();
            while !cancellation.is_cancelled() {
                thread::sleep(Duration::from_millis(5));
            }
            Err(AppError::new("AI_CANCELLED", "cancelled"))
        }
    }

    #[test]
    fn subtitle_queue_and_merge_follow_platform_resource_policy() {
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let (started_tx, started_rx) = mpsc::channel();
        let service = MediaJobService::with_resource_probe(
            manager,
            Arc::new(ImmediateExecutor),
            Arc::new(HoldingAIExecutor {
                started: started_tx,
            }),
            Arc::new(RecordingSink::default()),
            Box::new(|| scheduling::Resources {
                cores: 64,
                cpu_usage: Some(0.0),
                available_memory: Some(128 * scheduling::GIB),
                gpu: None,
            }),
        );
        let jobs: Vec<_> = (0..3)
            .map(|index| {
                let subtitle = index < 2;
                service
                    .start_ai(
                        StartAIJobRequest {
                            book_id: format!("mixed-{index}"),
                            title: "批量测试".into(),
                            series_root: fixture.series.clone(),
                            scope: MediaJobScope::Episodes,
                            inputs: vec![
                                fixture.write_input(&format!("mixed-{index}.mp4"), b"input")
                            ],
                            model: if subtitle { "small" } else { "htdemucs" }.into(),
                            device: "cpu".into(),
                        },
                        if subtitle {
                            MediaJobKind::ExtractSubtitles
                        } else {
                            MediaJobKind::SeparateBackgroundMusic
                        },
                    )
                    .unwrap()
            })
            .collect();
        if cfg!(target_os = "windows") {
            let mut started: Vec<_> = (0..3)
                .map(|_| started_rx.recv_timeout(Duration::from_secs(3)).unwrap())
                .collect();
            started.sort();
            assert_eq!(started, ["mixed-0", "mixed-1", "mixed-2"]);
        } else {
            for (index, job) in jobs.iter().enumerate() {
                assert_eq!(
                    started_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
                    format!("mixed-{index}")
                );
                assert!(started_rx.recv_timeout(Duration::from_millis(100)).is_err());
                if index < 2 {
                    service.cancel(&job.id).unwrap();
                    wait_for_job(&service, &job.id, MediaJobStatus::Cancelled);
                }
            }
        }
        let merge = service
            .start_merge(fixture.request(
                "exclusive-merge",
                vec![fixture.write_input("merge.mp4", b"input")],
            ))
            .unwrap();
        if cfg!(windows) {
            wait_for_job(&service, &merge.id, MediaJobStatus::Completed);
        } else {
            thread::sleep(Duration::from_millis(100));
            assert_eq!(
                service.manager.job(&merge.id).unwrap().status,
                MediaJobStatus::Queued
            );
        }
        for job in &jobs {
            if service.manager.job(&job.id).unwrap().status == MediaJobStatus::Running {
                service.cancel(&job.id).unwrap();
                wait_for_job(&service, &job.id, MediaJobStatus::Cancelled);
            }
        }
        wait_for_job(&service, &merge.id, MediaJobStatus::Completed);
    }

    #[cfg(windows)]
    #[test]
    fn queued_ai_at_its_limit_does_not_block_a_merge_behind_it() {
        let fixture = MergeFixture::new();
        let (tx, rx) = mpsc::channel();
        let service = MediaJobService::with_resource_probe_and_limit(
            Arc::new(MediaJobManager::load(&fixture.store).unwrap()),
            Arc::new(ImmediateExecutor),
            Arc::new(HoldingAIExecutor { started: tx }),
            Arc::new(RecordingSink::default()),
            Box::new(|| scheduling::Resources {
                cores: 16,
                cpu_usage: Some(0.0),
                available_memory: Some(32 * scheduling::GIB),
                gpu: None,
            }),
            1,
        );
        let mut jobs = Vec::new();
        for index in 0..2 {
            jobs.push(
                service
                    .start_ai(
                        StartAIJobRequest {
                            book_id: format!("pipeline-ai-{index}"),
                            title: "流水线".into(),
                            series_root: fixture.series.clone(),
                            scope: MediaJobScope::Episodes,
                            inputs: vec![fixture.write_input(&format!("ai-{index}.mp4"), b"input")],
                            model: "htdemucs".into(),
                            device: "cpu".into(),
                        },
                        MediaJobKind::SeparateBackgroundMusic,
                    )
                    .unwrap(),
            );
            if index == 0 {
                assert_eq!(
                    rx.recv_timeout(Duration::from_secs(3)).unwrap(),
                    "pipeline-ai-0"
                );
            }
        }
        let merge = service
            .start_merge(fixture.request(
                "pipeline-merge",
                vec![fixture.write_input("pipeline-merge.mp4", b"input")],
            ))
            .unwrap();
        wait_for_job(&service, &merge.id, MediaJobStatus::Completed);
        assert_eq!(
            service.manager.job(&jobs[0].id).unwrap().status,
            MediaJobStatus::Running
        );
        assert_eq!(
            service.manager.job(&jobs[1].id).unwrap().status,
            MediaJobStatus::Queued
        );
        service.cancel(&jobs[0].id).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            "pipeline-ai-1"
        );
        service.cancel(&jobs[1].id).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn configured_queue_refills_on_any_completion_and_lowering_does_not_cancel() {
        struct CompletingAI {
            started: mpsc::Sender<String>,
            completed: Arc<Mutex<HashSet<String>>>,
        }
        impl AIExecutor for CompletingAI {
            fn execute(
                &self,
                request: ValidatedAIJobRequest,
                token: &CancellationToken,
                _progress: &mut dyn FnMut(MergeProgress),
            ) -> Result<AIExecutionResult, AppError> {
                self.started.send(request.book_id.clone()).unwrap();
                loop {
                    if token.is_cancelled() {
                        return Err(AppError::new("AI_CANCELLED", "cancelled"));
                    }
                    if self.completed.lock().unwrap().contains(&request.book_id) {
                        return Ok(AIExecutionResult {
                            output_path: request.inputs[0].path.clone(),
                            outputs: Vec::new(),
                        });
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let (tx, rx) = mpsc::channel();
        let completed = Arc::new(Mutex::new(HashSet::new()));
        let service = MediaJobService::with_resource_probe_and_limit(
            manager,
            Arc::new(ImmediateExecutor),
            Arc::new(CompletingAI {
                started: tx,
                completed: completed.clone(),
            }),
            Arc::new(RecordingSink::default()),
            Box::new(|| scheduling::Resources {
                cores: 8,
                cpu_usage: Some(15.0),
                available_memory: Some(32 * scheduling::GIB),
                gpu: None,
            }),
            5,
        );
        let jobs: Vec<_> = (0..7)
            .map(|index| {
                service
                    .start_ai(
                        StartAIJobRequest {
                            book_id: format!("refill-{index}"),
                            title: "补位测试".into(),
                            series_root: fixture.series.clone(),
                            scope: MediaJobScope::Episodes,
                            inputs: vec![
                                fixture.write_input(&format!("refill-{index}.mp4"), b"input")
                            ],
                            model: "htdemucs".into(),
                            device: "cpu".into(),
                        },
                        MediaJobKind::SeparateBackgroundMusic,
                    )
                    .unwrap()
            })
            .collect();
        for _ in 0..5 {
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        completed.lock().unwrap().insert("refill-2".into());
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "refill-5");
        wait_for_job(&service, &jobs[2].id, MediaJobStatus::Completed);
        for index in [0, 1, 3, 4, 5] {
            assert_eq!(
                service.manager.job(&jobs[index].id).unwrap().status,
                MediaJobStatus::Running
            );
        }
        service.set_concurrency(2);
        for index in [0, 1, 3] {
            completed.lock().unwrap().insert(format!("refill-{index}"));
            wait_for_job(&service, &jobs[index].id, MediaJobStatus::Completed);
        }
        assert!(rx.recv_timeout(Duration::from_millis(150)).is_err());
        assert_eq!(
            service.manager.job(&jobs[4].id).unwrap().status,
            MediaJobStatus::Running
        );
        assert_eq!(
            service.manager.job(&jobs[5].id).unwrap().status,
            MediaJobStatus::Running
        );
        completed.lock().unwrap().insert("refill-5".into());
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "refill-6");
    }

    #[test]
    fn separation_queue_runs_five_and_cancel_only_releases_matching_slot() {
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let (started_tx, started_rx) = mpsc::channel();
        let resources = Arc::new(Mutex::new(scheduling::Resources {
            cores: 64,
            cpu_usage: Some(0.0),
            available_memory: Some(128 * scheduling::GIB),
            gpu: None,
        }));
        let probe = resources.clone();
        let service = MediaJobService::with_resource_probe(
            manager,
            Arc::new(ImmediateExecutor),
            Arc::new(HoldingAIExecutor {
                started: started_tx,
            }),
            Arc::new(RecordingSink::default()),
            Box::new(move || *probe.lock().unwrap()),
        );
        let jobs: Vec<_> = (0..6)
            .map(|index| {
                service
                    .start_ai(
                        StartAIJobRequest {
                            book_id: format!("parallel-{index}"),
                            title: "并发测试".into(),
                            series_root: fixture.series.clone(),
                            scope: MediaJobScope::Episodes,
                            inputs: vec![
                                fixture.write_input(&format!("parallel-{index}.mp4"), b"input")
                            ],
                            model: "htdemucs".into(),
                            device: "cpu".into(),
                        },
                        MediaJobKind::SeparateBackgroundMusic,
                    )
                    .unwrap()
            })
            .collect();
        let mut started: Vec<_> = (0..5)
            .map(|_| started_rx.recv_timeout(Duration::from_secs(3)).unwrap())
            .collect();
        started.sort();
        assert_eq!(
            started,
            (0..5)
                .map(|index| format!("parallel-{index}"))
                .collect::<Vec<_>>()
        );
        assert!(started_rx.recv_timeout(Duration::from_millis(100)).is_err());
        assert_eq!(
            service
                .snapshot()
                .jobs
                .iter()
                .filter(|j| j.status == MediaJobStatus::Running)
                .count(),
            5
        );
        service.pause(&jobs[1].id).unwrap();
        assert!(
            started_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "paused processes still hold resources"
        );
        service.resume(&jobs[1].id).unwrap();
        // Low memory must prevent replacement of a completed task, even with an empty slot.
        resources.lock().unwrap().available_memory = Some(scheduling::GIB);
        service.cancel(&jobs[2].id).unwrap();
        wait_for_job(&service, &jobs[2].id, MediaJobStatus::Cancelled);
        assert!(started_rx.recv_timeout(Duration::from_millis(100)).is_err());
        for index in [0, 1, 3, 4] {
            assert_eq!(
                service.manager.job(&jobs[index].id).unwrap().status,
                MediaJobStatus::Running
            );
        }
        resources.lock().unwrap().available_memory = Some(128 * scheduling::GIB);
        service.wake_worker();
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            "parallel-5"
        );
        service.delete(&jobs[1].id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while service.manager.job(&jobs[1].id).is_ok() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            service.manager.job(&jobs[5].id).unwrap().status,
            MediaJobStatus::Running
        );
        let retained = service.manager.clone();
        drop(service);
        assert!(retained
            .snapshot()
            .jobs
            .iter()
            .all(|j| j.status == MediaJobStatus::Cancelled));
    }

    #[test]
    fn completion_before_thread_exit_is_joined_before_releasing_its_slot() {
        let (finished_tx, finished_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (reaped_tx, reaped_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            finished_tx.send("finished".to_string()).unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(!worker.is_finished());
        let mut workers = HashMap::from([("finished".to_string(), worker)]);
        let running: RunningJobs = Arc::new(Mutex::new(HashMap::from([(
            "finished".to_string(),
            RunningJob {
                token: CancellationToken::new(),
                kind: MediaJobKind::Merge,
                budget: scheduling::ExecutionBudget {
                    cpu_threads: 1,
                    force_cpu: false,
                    merge: false,
                    memory: 0,
                    gpu_memory: 0,
                },
            },
        )])));
        let retained = running.clone();
        let reaper = thread::spawn(move || {
            reap_finished_workers(&mut workers, &running, &finished_rx);
            reaped_tx
                .send((workers.len(), running.lock().unwrap().len()))
                .unwrap();
        });
        // A completion notice must be consumed even while JoinHandle reports
        // false, but its resource slot must stay reserved until join finishes.
        assert!(reaped_rx.recv_timeout(Duration::from_millis(50)).is_err());
        assert_eq!(retained.lock().unwrap().len(), 1);
        release_tx.send(()).unwrap();
        assert_eq!(
            reaped_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            (0, 0)
        );
        reaper.join().unwrap();
    }

    #[test]
    fn worker_executes_two_jobs_oldest_first_without_overlap() {
        // Production mutation caught: spawning per-job workers or claiming a later queued job first.
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let executor = Arc::new(BlockingExecutor {
            started: started_tx,
            release: Mutex::new(release_rx),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        });
        let service = MediaJobService::with_resource_probe(
            manager,
            executor.clone(),
            Arc::new(UnavailableAIExecutor),
            Arc::new(RecordingSink::default()),
            Box::new(|| scheduling::Resources {
                cores: 8,
                cpu_usage: Some(0.0),
                available_memory: Some(16 * scheduling::GIB),
                gpu: None,
            }),
        );
        let first = service
            .start_merge(fixture.request("first", vec![fixture.write_input("first.mp4", b"first")]))
            .unwrap();
        let second = service
            .start_merge(
                fixture.request("second", vec![fixture.write_input("second.mp4", b"second")]),
            )
            .unwrap();

        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "first.mp4"
        );
        assert!(started_rx.recv_timeout(Duration::from_millis(100)).is_err());
        release_tx.send(()).unwrap();
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "second.mp4"
        );
        release_tx.send(()).unwrap();
        wait_for_job(&service, &first.id, MediaJobStatus::Completed);
        wait_for_job(&service, &second.id, MediaJobStatus::Completed);
        assert_eq!(executor.max_active.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn progress_events_are_complete_persisted_snapshots_and_terminal_precedes_next_claim() {
        // Production mutation caught: emitting deltas/pre-persist state or claiming the next job before terminal exposure.
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let sink = Arc::new(PersistCheckingSink {
            manager: manager.clone(),
            jobs: Mutex::new(Vec::new()),
            violations: AtomicUsize::new(0),
        });
        let service =
            MediaJobService::new(manager.clone(), Arc::new(ImmediateExecutor), sink.clone());
        let first = service
            .start_merge(fixture.request(
                "events-1",
                vec![fixture.write_input("events-1.mp4", b"one")],
            ))
            .unwrap();
        let second = service
            .start_merge(fixture.request(
                "events-2",
                vec![fixture.write_input("events-2.mp4", b"two")],
            ))
            .unwrap();
        wait_for_job(&service, &second.id, MediaJobStatus::Completed);

        // Completion is persisted before its event is delivered. Join the worker
        // before inspecting the final callback instead of racing that callback.
        drop(service);
        let events = sink.jobs.lock().unwrap().clone();
        assert_eq!(sink.violations.load(Ordering::SeqCst), 0);
        for event in &events {
            assert!(!event.id.is_empty());
            assert!(event.merge_request.is_some());
        }
        let first_terminal = events
            .iter()
            .position(|job| job.id == first.id && job.status == MediaJobStatus::Completed)
            .unwrap();
        let second_running = events
            .iter()
            .position(|job| job.id == second.id && job.status == MediaJobStatus::Running)
            .unwrap();
        assert!(first_terminal < second_running);
        let persisted = MediaJobManager::load(&fixture.store).unwrap().snapshot();
        assert_eq!(persisted.jobs, manager.snapshot().jobs);
    }

    struct CancellingExecutor {
        started: mpsc::Sender<()>,
    }

    impl MergeExecutor for CancellingExecutor {
        fn execute(
            &self,
            _request: MergeRequest,
            cancellation: &CancellationToken,
            _progress: &mut dyn FnMut(MergeProgress),
        ) -> Result<MergeResult, AppError> {
            self.started.send(()).unwrap();
            while !cancellation.is_cancelled() {
                thread::sleep(Duration::from_millis(5));
            }
            Err(AppError::new("MERGE_CANCELLED", "合并已取消"))
        }
    }

    #[test]
    fn running_cancel_signals_only_matching_token_returns_promptly_and_preserves_sources() {
        // Production mutation caught: synchronously waiting, cancelling a queued/different job, or deleting source media.
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let (started_tx, started_rx) = mpsc::channel();
        let service = MediaJobService::new(
            manager,
            Arc::new(CancellingExecutor {
                started: started_tx,
            }),
            Arc::new(RecordingSink::default()),
        );
        let input = fixture.write_input("cancel.mp4", b"keep-me");
        let job = service
            .start_merge(fixture.request("cancel", vec![input.clone()]))
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let started = Instant::now();
        let signalled = service.cancel(&job.id).unwrap();
        assert!(started.elapsed() < Duration::from_millis(200));
        assert_eq!(signalled.status, MediaJobStatus::Running);
        wait_for_job(&service, &job.id, MediaJobStatus::Cancelled);
        assert_eq!(fs::read(input.path).unwrap(), b"keep-me");
    }

    struct LateCancelAfterSuccessExecutor;

    impl MergeExecutor for LateCancelAfterSuccessExecutor {
        fn execute(
            &self,
            request: MergeRequest,
            cancellation: &CancellationToken,
            _progress: &mut dyn FnMut(MergeProgress),
        ) -> Result<MergeResult, AppError> {
            cancellation.cancel();
            Ok(MergeResult {
                output_path: request
                    .series_root
                    .join("合并视频")
                    .join(request.output_file_name),
            })
        }
    }

    #[test]
    fn executor_success_wins_over_cancel_signal_set_at_return_boundary() {
        // Production mutation caught: checking the cancellation token before honoring an already-returned successful merge result.
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let service = MediaJobService::new(
            manager,
            Arc::new(LateCancelAfterSuccessExecutor),
            Arc::new(RecordingSink::default()),
        );
        let job = service
            .start_merge(fixture.request(
                "late-cancel-success",
                vec![fixture.write_input("late-cancel.mp4", b"late")],
            ))
            .unwrap();

        let terminal = wait_for_terminal_job(&service, &job.id);

        assert_eq!(terminal.status, MediaJobStatus::Completed);
        let output_path = terminal
            .output_path
            .expect("completed merge should retain an output path");
        assert_eq!(
            output_path.file_name().and_then(|name| name.to_str()),
            Some("late-cancel-success.mp4")
        );
        assert!(output_path
            .components()
            .any(|component| component.as_os_str() == "合并视频"));
    }

    struct MissingToolsExecutor {
        root: PathBuf,
    }

    impl MergeExecutor for MissingToolsExecutor {
        fn execute(
            &self,
            _request: MergeRequest,
            _cancellation: &CancellationToken,
            _progress: &mut dyn FnMut(MergeProgress),
        ) -> Result<MergeResult, AppError> {
            match MediaTools::from_resource_root(&self.root) {
                Ok(_) => Err(AppError::new(
                    "TEST_UNEXPECTED_TOOL_FIXTURE",
                    "测试媒体工具意外可用",
                )),
                Err(error) => Err(error),
            }
        }
    }

    #[test]
    fn media_tool_failures_persist_and_emit_redacted_public_messages() {
        // Production mutation caught: persisting or emitting actual packaged/user resource paths from MediaTools errors.
        let fixture = MergeFixture::new();
        let private_tools_root = fixture.root.join("actual-user-private-media-tools");
        fs::create_dir_all(&private_tools_root).unwrap();
        let marker = private_tools_root.to_string_lossy().into_owned();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let sink = Arc::new(RecordingSink::default());
        let service = MediaJobService::new(
            manager,
            Arc::new(MissingToolsExecutor {
                root: private_tools_root,
            }),
            sink.clone(),
        );
        let job = service
            .start_merge(fixture.request(
                "missing-tools",
                vec![fixture.write_input("missing-tools.mp4", b"tools")],
            ))
            .unwrap();

        let failed = wait_for_job(&service, &job.id, MediaJobStatus::Failed);
        let serialized = serde_json::to_string(&failed).unwrap();
        let events = sink.jobs.lock().unwrap().clone();
        let failed_event = events
            .iter()
            .find(|event| event.id == job.id && event.status == MediaJobStatus::Failed)
            .expect("failed job event should be emitted");

        assert_eq!(failed.error_code.as_deref(), Some("MEDIA_TOOL_MISSING"));
        assert!(!failed
            .error_message
            .as_deref()
            .unwrap_or_default()
            .contains(&marker));
        assert!(!serialized.contains(&marker));
        assert!(!serde_json::to_string(failed_event)
            .unwrap()
            .contains(&marker));
    }

    struct ResilientExecutor;

    impl MergeExecutor for ResilientExecutor {
        fn execute(
            &self,
            request: MergeRequest,
            _cancellation: &CancellationToken,
            _progress: &mut dyn FnMut(MergeProgress),
        ) -> Result<MergeResult, AppError> {
            match request.output_file_name.as_str() {
                "error.mp4" => Err(AppError::new("FFMPEG_FAILED", "子进程失败")),
                "panic.mp4" => panic!("private panic detail"),
                _ => Ok(MergeResult {
                    output_path: request
                        .series_root
                        .join("合并视频")
                        .join(request.output_file_name),
                }),
            }
        }
    }

    #[test]
    fn executor_error_and_panic_fail_stably_while_worker_runs_later_jobs() {
        // Production mutation caught: allowing an executor error/panic to kill or poison the single worker.
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let service = MediaJobService::new(
            manager,
            Arc::new(ResilientExecutor),
            Arc::new(RecordingSink::default()),
        );
        let error = service
            .start_merge(fixture.request(
                "error",
                vec![fixture.write_input("error-input.mp4", b"error")],
            ))
            .unwrap();
        let panic_job = service
            .start_merge(fixture.request(
                "panic",
                vec![fixture.write_input("panic-input.mp4", b"panic")],
            ))
            .unwrap();
        let later = service
            .start_merge(fixture.request(
                "later",
                vec![fixture.write_input("later-input.mp4", b"later")],
            ))
            .unwrap();

        let error = wait_for_job(&service, &error.id, MediaJobStatus::Failed);
        let panic_job = wait_for_job(&service, &panic_job.id, MediaJobStatus::Failed);
        wait_for_job(&service, &later.id, MediaJobStatus::Completed);
        assert_eq!(error.error_code.as_deref(), Some("FFMPEG_FAILED"));
        assert_eq!(panic_job.error_code.as_deref(), Some("MEDIA_WORKER_FAILED"));
        assert!(!panic_job
            .error_message
            .as_deref()
            .unwrap_or_default()
            .contains("private panic"));
    }

    #[test]
    fn service_pauses_and_resumes_queued_and_running_jobs() {
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let (started_tx, started_rx) = mpsc::channel();
        let service = MediaJobService::new(
            manager,
            Arc::new(CancellingExecutor {
                started: started_tx,
            }),
            Arc::new(RecordingSink::default()),
        );
        let running = service
            .start_merge(fixture.request(
                "pause-running",
                vec![fixture.write_input("pause-running.mp4", b"running")],
            ))
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let queued = service
            .start_merge(fixture.request(
                "pause-queued",
                vec![fixture.write_input("pause-queued.mp4", b"queued")],
            ))
            .unwrap();

        let queued_paused = service.pause(&queued.id).unwrap();
        assert_eq!(queued_paused.status, MediaJobStatus::Paused);
        assert_eq!(
            queued_paused.pause_origin,
            Some(MediaJobPauseOrigin::Queued)
        );
        let queued_resumed = service.resume(&queued.id).unwrap();
        assert_eq!(queued_resumed.status, MediaJobStatus::Queued);

        let running_paused = service.pause(&running.id).unwrap();
        assert_eq!(running_paused.status, MediaJobStatus::Paused);
        assert_eq!(
            running_paused.pause_origin,
            Some(MediaJobPauseOrigin::Running)
        );
        let running_resumed = service.resume(&running.id).unwrap();
        assert_eq!(running_resumed.status, MediaJobStatus::Running);
        service.cancel(&running.id).unwrap();
    }

    #[test]
    fn deleting_running_job_removes_record_after_worker_stops_and_keeps_source() {
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let (started_tx, started_rx) = mpsc::channel();
        let service = MediaJobService::new(
            manager,
            Arc::new(CancellingExecutor {
                started: started_tx,
            }),
            Arc::new(RecordingSink::default()),
        );
        let input = fixture.write_input("delete-running.mp4", b"keep-source");
        let job = service
            .start_merge(fixture.request("delete-running", vec![input.clone()]))
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        service.delete(&job.id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while service.snapshot().jobs.iter().any(|item| item.id == job.id) {
            assert!(Instant::now() < deadline, "deleted job record remained");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(fs::read(input.path).unwrap(), b"keep-source");
    }

    #[test]
    fn merged_mp4_blocks_new_merge_until_file_is_removed() {
        let fixture = MergeFixture::new();
        let manager = Arc::new(MediaJobManager::load(&fixture.store).unwrap());
        let service = MediaJobService::new(
            manager,
            Arc::new(ImmediateExecutor),
            Arc::new(RecordingSink::default()),
        );
        let merged_dir = fixture.series.join("合并视频");
        fs::create_dir(&merged_dir).unwrap();
        let existing = merged_dir.join("existing.MP4");
        fs::write(&existing, b"merged").unwrap();
        let input = fixture.write_input("guard-input.mp4", b"input");
        let request = fixture.request("guard", vec![input]);

        assert!(service.has_merged_video(&fixture.series).unwrap());
        assert_eq!(
            service.start_merge(request.clone()).unwrap_err().code,
            "MERGE_OUTPUT_EXISTS"
        );
        fs::remove_file(existing).unwrap();
        assert!(!service.has_merged_video(&fixture.series).unwrap());
        assert_eq!(
            service.start_merge(request).unwrap().status,
            MediaJobStatus::Queued
        );
    }

    #[test]
    fn has_merged_video_returns_false_when_series_root_is_missing() {
        let missing = std::env::temp_dir().join(format!(
            "hongguo-missing-series-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        assert!(!has_merged_video_in_series(&missing).unwrap());
        assert_eq!(find_merged_video_in_series(&missing).unwrap(), None);
    }

    #[test]
    fn has_merged_video_returns_false_when_merge_directory_is_unsafe() {
        let fixture = MergeFixture::new();
        fs::write(fixture.series.join("合并视频"), b"not-a-directory").unwrap();
        assert!(!has_merged_video_in_series(&fixture.series).unwrap());
        assert_eq!(find_merged_video_in_series(&fixture.series).unwrap(), None);
    }

    #[test]
    fn find_merged_video_returns_canonical_mp4_inside_series() {
        let fixture = MergeFixture::new();
        let merged_dir = fixture.series.join("合并视频");
        fs::create_dir(&merged_dir).unwrap();
        let existing = merged_dir.join("成片.mp4");
        fs::write(&existing, b"merged").unwrap();
        let found = find_merged_video_in_series(&fixture.series)
            .unwrap()
            .expect("merged mp4");
        assert_eq!(found, fs::canonicalize(&existing).unwrap());
        assert!(has_merged_video_in_series(&fixture.series).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn validate_ai_accepts_lexical_tmp_series_with_canonical_merged_file() {
        if !Path::new("/tmp").exists() {
            return;
        }
        let unique = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let lexical_series = PathBuf::from("/tmp").join(format!(
            "hongguo-ai-tmp-{}-{}-{unique}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&lexical_series).expect("tmp series");
        let merged_dir = lexical_series.join("合并视频");
        fs::create_dir(&merged_dir).unwrap();
        let merged = merged_dir.join("merged.mp4");
        fs::write(&merged, b"merged").unwrap();
        let canonical_merged = fs::canonicalize(&merged).unwrap();
        let request = StartAIJobRequest {
            book_id: "book-tmp".into(),
            title: "tmp剧".into(),
            series_root: lexical_series.clone(),
            scope: MediaJobScope::Merged,
            inputs: vec![StartMergeInput {
                episode_index: 1,
                path: canonical_merged.clone(),
            }],
            model: "htdemucs".into(),
            device: "auto".into(),
        };
        validate_ai_request(&request, MediaJobKind::SeparateBackgroundMusic)
            .expect("lexical tmp series should accept canonical merged input");
        assert_eq!(
            find_merged_video_in_series(&lexical_series)
                .unwrap()
                .as_deref(),
            Some(canonical_merged.as_path())
        );
        let _ = fs::remove_dir_all(&lexical_series);
    }
}
