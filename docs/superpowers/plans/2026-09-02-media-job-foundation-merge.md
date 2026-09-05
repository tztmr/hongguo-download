# Media Job Foundation and Video Merge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add persistent native media jobs, self-contained release tooling, and safe episode-order video merge/H.264 conversion to the existing download manager.

**Architecture:** React sends immutable completed-download snapshots to focused Tauri commands. Rust owns media-job persistence, cancellation, subprocess execution, temp directories, ffprobe validation, and atomic publication; bundled FFmpeg/ffprobe and a packaged API sidecar remove release-time system dependencies.

**Tech Stack:** React 19, TypeScript 5.8, Vitest, Tauri 2.8, Rust 2021, serde, tokio, FFmpeg/ffprobe, PyInstaller, shell build scripts.

**Spec:** `docs/superpowers/specs/2026-09-02-media-processing-youtube-upload-design.md`

## Global Constraints

- `合并视频`、`分离背景音乐`、`提取字幕`必须始终是独立任务和按钮。
- 合并必须按 `episodeIndex` 升序；H.264 默认开启。
- 无转码仅在视频/音频流参数兼容时允许；不兼容时必须阻断。
- H.264 优先 `h264_videotoolbox`，失败回退 `libx264`，音频输出 AAC。
- 媒体处理默认一次只运行一个重型任务，不占用现有下载并发额度。
- 任务取消、失败、中断和删除记录均不得删除原始下载 MP4。
- 正式输出只能在 ffprobe 验证通过后从任务临时目录原子移入。
- 打包的 `.app` 不得依赖系统 Python 或系统 FFmpeg。
- 代码示例中的测试夹具/构造器必须在对应测试文件和任务内一并定义；它们不是可从现有项目导入的隐藏接口。
- 本阶段优先验证 macOS ARM64；Windows、macOS Intel、签名和公证不作为完成证据。
- 当前工作区不是 Git 仓库；每个“检查点”记录测试与实际文件差异，不初始化仓库或执行 `git commit`。

## File Structure

### React

- Modify `desktop/src/download/model.ts`: persist series upload metadata on each batch.
- Modify `desktop/src/download/model.test.ts`: metadata capture and migration-facing behavior.
- Modify `desktop/src/download/storage.ts`: migrate download state v1 to v2 without losing completed paths.
- Modify `desktop/src/download/storage.test.ts`: v1 fixture migration.
- Create `desktop/src/media/types.ts`: frontend media-job/merge request contracts.
- Create `desktop/src/media/commands.ts`: typed Tauri command and event adapter.
- Create `desktop/src/media/useMediaJobs.ts`: native snapshot load, progress subscription, start/cancel/retry actions.
- Create `desktop/src/media/useMediaJobs.test.tsx`: hook contract tests.
- Create `desktop/src/components/MergeVideoDialog.tsx` and `.test.tsx`: episode list, output name, H.264 default.
- Modify `desktop/src/components/DownloadManagerPage.tsx` and `.test.tsx`: media tabs, completed-batch action, job rows.
- Modify `desktop/src/App.tsx`, `desktop/src/preview.ts`, `desktop/src/styles.css`: compose adapter and deterministic preview.

### Rust and release tooling

- Create `desktop/src-tauri/src/app_error.rs`: stable structured error shared by commands.
- Create `desktop/src-tauri/src/media/mod.rs`: command registration and manager exports.
- Create `desktop/src-tauri/src/media/model.rs`: job/request/progress/result types.
- Create `desktop/src-tauri/src/media/storage.rs`: versioned atomic job store and restart recovery.
- Create `desktop/src-tauri/src/media/tools.rs`: resource resolver, subprocess cancellation, ffprobe/ffmpeg helpers.
- Create `desktop/src-tauri/src/media/merge.rs`: compatibility check and merge execution.
- Modify `desktop/src-tauri/src/lib.rs`: split app state, start packaged API sidecar, manage media jobs, register commands.
- Modify `desktop/src-tauri/Cargo.toml`, `desktop/src-tauri/tauri.conf.json`, `desktop/src-tauri/capabilities/default.json`: sidecar/tool dependencies and resources.
- Create `scripts/build-api-sidecar.sh`: build target-triple-named API executable.
- Create `scripts/stage-media-tools.sh`: validate caller-supplied release FFmpeg/ffprobe plus SHA-256 before staging.
- Create `requirements-build.txt`: reproducible API sidecar build dependency range.

---

### Task 1: Download Batch Metadata and v2 Migration

**Files:**
- Modify: `desktop/src/download/model.ts`
- Modify: `desktop/src/download/model.test.ts`
- Modify: `desktop/src/download/storage.ts`
- Modify: `desktop/src/download/storage.test.ts`

**Interfaces:**
- Produces: `DownloadSeriesSnapshot` and `DownloadManagerState.version === 2`.
- Produces: completed batch fields required by later plans: `seriesId`, `abstract`, `category`, `contentTypeCode`.
- Preserves: all v1 queue statuses, completed paths, concurrency, pause state, and notification marker.

- [ ] **Step 1: Write failing metadata and migration tests**

```ts
it("captures immutable series metadata for later upload defaults", () => {
  const result = enqueueEpisodes(createInitialState(), series, [episode], "auto", fixedIds, 1000);
  expect(result.state.batches[0].series).toEqual({
    bookId: series.bookId,
    seriesId: series.seriesId,
    title: series.title,
    cover: series.cover,
    abstract: series.abstract,
    category: series.category,
    contentTypeCode: series.contentTypeCode,
  });
});

it("migrates v1 completed paths into v2 without resetting done items", () => {
  const restored = loadDownloadState(memoryStorageWith(v1Fixture));
  expect(restored.state.version).toBe(2);
  expect(restored.state.batches[0].items[0]).toMatchObject({ status: "done", path: "/tmp/e1.mp4" });
});
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run: `cd desktop && npm test -- src/download/model.test.ts src/download/storage.test.ts`

Expected: FAIL because the state is still v1 and `DownloadBatch.series` does not exist.

- [ ] **Step 3: Implement the v2 model and explicit v1 migration**

```ts
export type DownloadSeriesSnapshot = Pick<
  SeriesItem,
  "bookId" | "seriesId" | "title" | "cover" | "abstract" | "category" | "contentTypeCode"
>;

export type DownloadManagerState = {
  version: 2;
  concurrency: number;
  globallyPaused: boolean;
  batches: DownloadBatch[];
};
```

`loadDownloadState()` must accept versions 1 and 2. For v1, derive `series` from `bookId/title/cover`, use empty abstract/category and `contentTypeCode: 1`, then persist as v2 on the next normal state write. Never invent a missing cover or abstract.

- [ ] **Step 4: Run download-domain regression tests**

Run: `cd desktop && npm test -- src/download/model.test.ts src/download/storage.test.ts src/download/useDownloadManager.test.tsx`

Expected: PASS; existing concurrency, pause, retry, notification, and path behavior remains unchanged.

- [ ] **Step 5: Checkpoint**

Run: `cd desktop && npm run build`

Expected: PASS. Record changed files and test counts; do not perform Git actions.

---

### Task 2: Stable Rust Errors, Media Job Model, and Atomic Storage

**Files:**
- Create: `desktop/src-tauri/src/app_error.rs`
- Create: `desktop/src-tauri/src/media/mod.rs`
- Create: `desktop/src-tauri/src/media/model.rs`
- Create: `desktop/src-tauri/src/media/storage.rs`
- Modify: `desktop/src-tauri/src/lib.rs`

**Interfaces:**
- Produces: `AppError { code: String, message: String }` serialized without internal causes.
- Produces: `MediaJobManager::load(path)`, `snapshot()`, `enqueue(request)`, `cancel(id)`, `retry(id)`, `update(id, transition)`.
- Produces commands: `get_media_jobs`, `start_merge_job`, `cancel_media_job`, `retry_media_job`.

- [ ] **Step 1: Write Rust unit tests for state transitions and recovery**

```rust
#[test]
fn running_jobs_restore_as_interrupted_without_touching_inputs() {
    let store = fixture_store_with(MediaJobStatus::Running);
    let manager = MediaJobManager::load(store.path()).unwrap();
    let job = &manager.snapshot().jobs[0];
    assert_eq!(job.status, MediaJobStatus::Interrupted);
    assert!(store.input_path().exists());
}

#[test]
fn duplicate_active_job_key_is_rejected() {
    let mut manager = empty_manager();
    manager.enqueue(merge_request("book-1", "snapshot-1")).unwrap();
    let error = manager.enqueue(merge_request("book-1", "snapshot-1")).unwrap_err();
    assert_eq!(error.code, "MEDIA_JOB_ALREADY_ACTIVE");
}
```

- [ ] **Step 2: Run Rust tests and verify RED**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml media::`

Expected: compile failure because the media modules do not exist.

- [ ] **Step 3: Implement the job types and state machine**

```rust
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaJobStatus { Queued, Running, Completed, Failed, Cancelled, Interrupted }

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaJob {
    pub id: String,
    pub dedupe_key: String,
    pub kind: MediaJobKind,
    pub status: MediaJobStatus,
    pub stage: String,
    pub percent: f64,
    pub inputs: Vec<InputSnapshot>,
    pub output_path: Option<PathBuf>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}
```

Allow only `queued -> running`, `running -> completed|failed|cancelled`, and `failed|cancelled|interrupted -> queued`. Reject all other transitions with `MEDIA_JOB_INVALID_TRANSITION`.

- [ ] **Step 4: Implement versioned atomic persistence**

Write `media-jobs.json.tmp`, `sync_all`, rename to `media-jobs.json`, and sync the parent directory. On load, change only `running` to `interrupted`; keep completed outputs and failed diagnostics. If JSON is corrupt, rename a copy to `media-jobs.corrupt.<unix>.json`, start empty, and return a warning.

- [ ] **Step 5: Register read/cancel/retry commands with an idle worker queue**

`AppState` owns `Arc<MediaJobManager>`. Queue execution uses one worker and a cancellation token per running job. `cancel_media_job` signals only the selected job and returns immediately; it must not delete files directly.

- [ ] **Step 6: Run tests and checkpoint**

Run: `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml -- --check && cargo test --manifest-path desktop/src-tauri/Cargo.toml media::`

Expected: PASS with deterministic storage fixtures and no real subprocesses.

---

### Task 3: Self-Contained API Sidecar and Bundled Tool Staging

**Files:**
- Create: `requirements-build.txt`
- Create: `scripts/build-api-sidecar.sh`
- Create: `scripts/stage-media-tools.sh`
- Modify: `desktop/src-tauri/Cargo.toml`
- Modify: `desktop/src-tauri/tauri.conf.json`
- Modify: `desktop/src-tauri/capabilities/default.json`
- Modify: `desktop/src-tauri/src/lib.rs`

**Interfaces:**
- Produces release executables in `desktop/src-tauri/binaries/` named for `aarch64-apple-darwin`.
- Consumes required release inputs `HONGGUO_FFMPEG_ARCHIVE`, `HONGGUO_FFMPEG_SHA256`, and `HONGGUO_FFPROBE_SHA256`; the staging script fails closed when any is absent or mismatched.
- Produces `spawn_api()` that uses the packaged `hongguo-api` sidecar in release builds.

- [ ] **Step 1: Add failing resource-resolution tests**

```rust
#[test]
fn release_tool_resolution_never_falls_back_to_path() {
    let root = fixture_resource_root();
    let error = MediaTools::from_resource_root(&root).unwrap_err();
    assert_eq!(error.code, "MEDIA_TOOL_MISSING");
}
```

Also test executable-bit rejection and a valid fixture pair.

- [ ] **Step 2: Implement reproducible staging scripts**

`build-api-sidecar.sh` creates an isolated venv, installs `requirements.txt` plus `requirements-build.txt`, runs PyInstaller against `main.py`, renames the result `hongguo-api-aarch64-apple-darwin`, and executes `--health-probe` before staging. Add a guarded `--health-probe` path to `main.py` that imports the production app and exits 0 without listening.

`stage-media-tools.sh` must:

```bash
: "${HONGGUO_FFMPEG_ARCHIVE:?required}"
: "${HONGGUO_FFMPEG_SHA256:?required}"
: "${HONGGUO_FFPROBE_SHA256:?required}"
shasum -a 256 -c "$checksum_file"
```

It extracts only explicit `ffmpeg` and `ffprobe` members, checks Mach-O ARM64 architecture and executable bits, runs `-version`, then stages target-triple filenames. It must not copy Homebrew dynamic binaries into the DMG.

- [ ] **Step 3: Configure Tauri sidecars and permissions**

Add `tauri-plugin-shell` to Rust/JS dependencies, `shell:allow-spawn` and `shell:allow-kill` capabilities, and these `bundle.externalBin` entries:

```json
["binaries/hongguo-api", "binaries/ffmpeg", "binaries/ffprobe"]
```

Use shell sidecar APIs to spawn the API with `--host 127.0.0.1 --port <reserved> --data-dir <app-data>`. Development may explicitly use `python3 -B -m uvicorn`; release code must not call `which_python()`.

- [ ] **Step 4: Replace release Python discovery tests**

Delete tests that assert a system Python has packages. Add tests for sidecar argument construction, health contract, shutdown, and release-mode missing-sidecar errors that do not mention `pip install`.

- [ ] **Step 5: Run focused checks**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml api_sidecar media::tools`

Expected: PASS. Run the staging scripts with synthetic wrong checksums and confirm non-zero exit before testing valid release assets.

- [ ] **Step 6: Checkpoint**

Run: `cd desktop && npm run build && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`

Expected: PASS; no real OAuth credential or downloaded model is present under `desktop/src-tauri/binaries`.

---

### Task 4: ffprobe Contracts and Safe Merge Execution

**Files:**
- Create: `desktop/src-tauri/src/media/tools.rs`
- Create: `desktop/src-tauri/src/media/merge.rs`
- Modify: `desktop/src-tauri/src/media/model.rs`
- Modify: `desktop/src-tauri/src/media/mod.rs`

**Interfaces:**
- Produces: `probe_media(path) -> MediaProbe`, `can_stream_copy(&[MediaProbe]) -> bool`.
- Produces: `run_merge(tools, request, cancellation, progress) -> MergeResult`.
- Consumes: sorted `MergeInput { episode_index, path, size, modified_unix_nanos }`.

- [ ] **Step 1: Write failing compatibility and command tests**

```rust
#[test]
fn stream_copy_requires_matching_video_and_audio_signatures() {
    assert!(can_stream_copy(&[probe("h264", 1080, "aac", 48000), probe("h264", 1080, "aac", 48000)]));
    assert!(!can_stream_copy(&[probe("h264", 1080, "aac", 48000), probe("hevc", 1080, "aac", 48000)]));
}

#[test]
fn inputs_are_sorted_by_episode_index_before_concat_file_is_written() {
    let lines = concat_lines(&[input(2, "b.mp4"), input(1, "a.mp4")]).unwrap();
    assert_eq!(lines, vec!["file 'a.mp4'", "file 'b.mp4'"]);
}
```

Also test quote/newline rejection in concat paths, H.264 hardware command, `libx264` fallback, and output-path containment.

- [ ] **Step 2: Run and verify RED**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml media::merge`

Expected: compile failure for missing merge helpers.

- [ ] **Step 3: Implement JSON ffprobe parsing and compatibility**

```rust
#[derive(Clone, PartialEq, Eq)]
pub struct StreamSignature {
    pub codec_name: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<String>,
    pub time_base: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
}
```

Require exactly one primary video stream; compare video and optional audio signatures. If `transcode_h264 == false` and signatures differ, return `MERGE_TRANSCODE_REQUIRED` before creating a job output.

- [ ] **Step 4: Implement cancellable subprocess execution**

Pass arguments as an array, never through a shell. Parse `-progress pipe:1 -nostats` key/value lines and emit `media-job-progress` at most every 200ms plus every stage change. On cancellation, terminate the child, wait for exit, and remove only the job temp directory.

- [ ] **Step 5: Implement hardware-first merge and atomic validation**

For H.264, try `-c:v h264_videotoolbox -c:a aac`; only recognized encoder initialization/runtime failures retry once with `-c:v libx264 -c:a aac`. Run ffprobe on the completed temp file, require video stream, expected optional audio stream, and duration within `max(1.0s, expected * 0.01)`, then rename into `<series>/合并视频/`.

- [ ] **Step 6: Run generated-fixture integration tests**

Generate two one-second color/tone clips with the staged FFmpeg, then test stream copy, incompatible rejection, H.264 output, cancellation, and source-file survival.

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml media::merge -- --nocapture`

Expected: PASS; ffprobe can read every completed fixture output.

- [ ] **Step 7: Checkpoint**

Run: `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml -- --check && cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --all-targets -- -D warnings`

Expected: PASS.

---

### Task 5: Native Merge Commands and Worker Queue

**Files:**
- Modify: `desktop/src-tauri/src/media/mod.rs`
- Modify: `desktop/src-tauri/src/media/model.rs`
- Modify: `desktop/src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: `StartMergeRequest` from the frontend.
- Produces Tauri commands: `get_media_jobs`, `start_merge_job`, `cancel_media_job`, `retry_media_job`.
- Emits: `media-job-progress` with complete `MediaJob` snapshots.

- [ ] **Step 1: Write command-boundary tests**

```rust
#[test]
fn start_merge_rejects_non_done_or_changed_inputs() {
    let request = merge_request_with_missing_input();
    let error = validate_merge_request(&request).unwrap_err();
    assert_eq!(error.code, "MEDIA_INPUT_CHANGED");
}
```

Test no inputs, duplicate episode indices, output outside the series directory, active duplicate, and retry preserving the original request.

- [ ] **Step 2: Implement strict request validation**

Snapshot each input with canonical path, file size, and modification nanoseconds. Derive the dedupe key from job kind, book ID, input snapshots, scope, and `transcodeH264`; never trust a frontend-provided dedupe key or output path outside the selected series folder.

- [ ] **Step 3: Connect the single-worker scheduler**

After enqueue, wake the worker; atomically claim the oldest queued job, mark running, execute, persist terminal status, emit terminal progress, and then claim the next job. A panic or child crash becomes `MEDIA_WORKER_FAILED` and leaves the manager alive.

- [ ] **Step 4: Run command and manager tests**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml media::`

Expected: PASS; two queued jobs execute sequentially while download tests remain unchanged.

- [ ] **Step 5: Checkpoint**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml`

Expected: all Rust tests PASS.

---

### Task 6: React Media Adapter, Merge Dialog, and Download Manager UI

**Files:**
- Create: `desktop/src/media/types.ts`
- Create: `desktop/src/media/commands.ts`
- Create: `desktop/src/media/useMediaJobs.ts`
- Create: `desktop/src/media/useMediaJobs.test.tsx`
- Create: `desktop/src/components/MergeVideoDialog.tsx`
- Create: `desktop/src/components/MergeVideoDialog.test.tsx`
- Modify: `desktop/src/components/DownloadManagerPage.tsx`
- Modify: `desktop/src/components/DownloadManagerPage.test.tsx`
- Modify: `desktop/src/App.tsx`
- Modify: `desktop/src/preview.ts`
- Modify: `desktop/src/styles.css`

**Interfaces:**
- Produces: `MediaJobsModel { jobs, warning, startMerge, cancel, retry }`.
- `startMerge(batch, { outputName, transcodeH264, conflictPolicy })` consumes only completed items with paths.
- Download manager receives `media: MediaJobsModel` without importing Tauri APIs directly.

- [ ] **Step 1: Write hook and dialog tests**

```tsx
it("defaults H.264 on and submits episode-sorted completed paths", async () => {
  render(<MergeVideoDialog batch={batchWithOutOfOrderItems} onSubmit={submit} onClose={noop} />);
  expect(screen.getByRole("checkbox", { name: "转为 H.264" })).toBeChecked();
  fireEvent.click(screen.getByRole("button", { name: "开始合并" }));
  expect(submit).toHaveBeenCalledWith(expect.objectContaining({
    transcodeH264: true,
    inputs: [expect.objectContaining({ episodeIndex: 1 }), expect.objectContaining({ episodeIndex: 2 })],
  }));
});
```

Test disabled action for incomplete batches, event subscription cleanup, cancel/retry routing, duplicate-active error display, and conflict choice defaulting to cancel/no overwrite.

- [ ] **Step 2: Run and verify RED**

Run: `cd desktop && npm test -- src/media/useMediaJobs.test.tsx src/components/MergeVideoDialog.test.tsx`

Expected: FAIL for missing modules/components.

- [ ] **Step 3: Implement typed commands and hook**

```ts
export const mediaCommands = {
  snapshot: () => invoke<MediaJobSnapshot>("get_media_jobs"),
  startMerge: (request: StartMergeRequest) => invoke<MediaJob>("start_merge_job", { request }),
  cancel: (jobId: string) => invoke<void>("cancel_media_job", { jobId }),
  retry: (jobId: string) => invoke<MediaJob>("retry_media_job", { jobId }),
};
```

`useMediaJobs` loads the native snapshot once, listens to `media-job-progress`, replaces jobs by ID, and exposes normalized stable-code errors. Never duplicate the native scheduler in React.

- [ ] **Step 4: Add the three download-manager sections**

Add `下载任务`, `媒体处理`, and `YouTube 上传` tabs. This plan populates download and media tabs; the upload tab may render the explicit empty copy `尚无上传任务` until the YouTube plan. Completed batches show `合并视频`; reserve separate disabled slots or layout space for later independent buttons without combining them.

- [ ] **Step 5: Implement job rows and preview fixtures**

Render status, stage, percent, output path, safe error, cancel, retry, and locate actions. Add deterministic queued/running/completed preview jobs so browser validation does not invoke native commands.

- [ ] **Step 6: Run frontend tests and build**

Run: `cd desktop && npm test -- src/media/useMediaJobs.test.tsx src/components/MergeVideoDialog.test.tsx src/components/DownloadManagerPage.test.tsx && npm run build`

Expected: PASS with no act warnings or Tauri calls in preview mode.

- [ ] **Step 7: Browser checkpoint**

Open `http://localhost:1424/?preview=downloads`; verify 1440×900 and the minimum-width layout, H.264 default, tabs, progress rows, cancel/retry, and no clipped dialog controls.

---

### Task 7: Plan-1 Regression and Packaged Smoke Check

**Files:**
- Modify only files already listed when a failing acceptance check requires a fix.

**Interfaces:**
- Validates all interfaces produced by Tasks 1–6 for the next AI and YouTube plans.

- [ ] **Step 1: Run the full automated suite**

Run:

```bash
python3 -m unittest discover -s tests -v
cd desktop && npm test && npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: every command exits 0.

- [ ] **Step 2: Build the API sidecar and stage verified media tools**

Run the two scripts with explicit local release artifact/checksum variables. Expected: target-triple binaries exist, are ARM64 executables, and `ffmpeg -encoders` includes `h264_videotoolbox` plus `libx264`.

- [ ] **Step 3: Build and inspect the DMG**

Run: `cd desktop && npm run tauri build`

Expected: `.app` and `.dmg` are produced. Inspect the bundle for API sidecar, ffmpeg, ffprobe, and license notices; confirm it contains no OAuth JSON, token, or AI model.

- [ ] **Step 4: Launch packaged app and execute one merge**

Launch the packaged `.app` with a deliberately minimal `PATH`. Confirm local API health, an existing download remains visible, H.264 merge completes, output is ffprobe-readable, and no system-Python/system-FFmpeg error appears.

- [ ] **Step 5: Record checkpoint**

Record artifact paths, checksums, automated counts, browser result, packaged runtime proof, and all unverified platform/signing layers. Do not claim AI or YouTube behavior from this plan.
