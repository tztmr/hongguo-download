# Media Job Pause, Delete, and Merge Guard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add real pause/resume and record deletion to media jobs, and prevent a new merge whenever the series already contains a merged MP4.

**Architecture:** Extend the persisted media state machine with pause origin, add a shared POSIX process controller to the existing cancellation token, and let the service coordinate deferred deletion of an active worker. Treat the filesystem under `合并视频/` as the source of truth for the merge guard, enforcing it in Rust and reflecting it in React.

**Tech Stack:** Rust, Tauri 2, POSIX process groups through `libc`, React 19, TypeScript, Vitest, Testing Library.

**Spec:** `docs/superpowers/specs/2026-09-06-media-job-pause-delete-merge-guard-design.md`

## Global Constraints

- Applies to merge, background-music separation, and subtitle extraction jobs.
- Running FFmpeg, Demucs, and Whisper work must stop through `SIGSTOP` and continue through `SIGCONT` in the same app session.
- `paused(origin=queued)` survives restart; `running` and `paused(origin=running)` recover as `interrupted`.
- Deleting a media job must never delete downloaded source files or published media outputs.
- Any regular, non-symlink `.mp4` directly inside the real `合并视频/` directory blocks a new merge.
- Keep deserialization compatibility for old `overwrite` requests, but all new merge requests use `failIfExists`.
- Preserve all unrelated dirty worktree changes and stage only files changed by each task.

---

### Task 1: Persisted pause state and job deletion

**Files:**
- Modify: `desktop/src-tauri/src/media/model.rs`
- Modify: `desktop/src-tauri/src/media/storage.rs`

**Interfaces:**
- Produces: `MediaJobStatus::Paused`, `MediaJobPauseOrigin::{Queued,Running}`, `MediaJob.pause_origin: Option<MediaJobPauseOrigin>`.
- Produces: `MediaJobTransition::{PauseQueued,PauseRunning,ResumeQueued,ResumeRunning}`.
- Produces: `MediaJobManager::{pause_queued,pause_running,resume_queued,resume_running,remove}`.
- Consumes: existing atomic snapshot persistence in `persist_jobs`.

- [ ] **Step 1: Write failing state-machine tests**

Add tests in `storage.rs` that construct queued and running jobs and assert exact transitions:

```rust
assert_eq!(manager.pause_queued(&queued.id)?.status, MediaJobStatus::Paused);
assert_eq!(manager.pause_queued(&queued.id)?.pause_origin, Some(MediaJobPauseOrigin::Queued));
assert_eq!(manager.resume_queued(&queued.id)?.status, MediaJobStatus::Queued);

let running = manager.claim_oldest_queued()?.unwrap();
assert_eq!(manager.pause_running(&running.id)?.pause_origin, Some(MediaJobPauseOrigin::Running));
assert_eq!(manager.resume_running(&running.id)?.status, MediaJobStatus::Running);
```

Also test that `remove` persists absence, invalid transitions return `MEDIA_JOB_INVALID_TRANSITION`, paused jobs participate in active deduplication, and progress updates are accepted for `Paused + Running` but rejected for `Paused + Queued`.

- [ ] **Step 2: Run the focused Rust tests and verify failure**

Run:

```bash
cd desktop/src-tauri
cargo test media::storage --lib
```

Expected: compilation fails because paused types and manager methods do not exist.

- [ ] **Step 3: Implement the persisted state model**

Add camel-case serialized types and fields:

```rust
pub enum MediaJobStatus { Queued, Running, Paused, Completed, Failed, Cancelled, Interrupted }
pub enum MediaJobPauseOrigin { Queued, Running }

pub struct MediaJob {
    // existing fields
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_origin: Option<MediaJobPauseOrigin>,
}
```

Initialize `pause_origin` to `None`, clear it on all non-pause transitions, include `Paused` in active dedupe checks, and add transition helpers with the exact signatures from the interface block.

On load, convert `Running` and `Paused + Running` to `Interrupted`; preserve `Paused + Queued`. Treat a legacy `Paused` record with no origin as `Interrupted` so a malformed snapshot cannot enter the queue.

- [ ] **Step 4: Implement atomic record removal**

Add:

```rust
pub fn remove(&self, id: &str) -> Result<MediaJob, AppError>
```

Clone the snapshot, remove exactly one matching job, persist the clone, then replace in-memory state. Return `MEDIA_JOB_NOT_FOUND` if absent.

- [ ] **Step 5: Run state and persistence tests**

Run:

```bash
cd desktop/src-tauri
cargo test media::storage --lib
cargo test media::model --lib
```

Expected: all focused tests pass.

- [ ] **Step 6: Commit the state-model change**

```bash
git add desktop/src-tauri/src/media/model.rs desktop/src-tauri/src/media/storage.rs
git commit -m "feat: add paused media job state"
```

### Task 2: Shared POSIX process control

**Files:**
- Create: `desktop/src-tauri/src/media/process_control.rs`
- Modify: `desktop/src-tauri/src/media/mod.rs`
- Modify: `desktop/src-tauri/src/media/merge.rs`
- Modify: `desktop/src-tauri/src/media/ai.rs`

**Interfaces:**
- Consumes: `libc = "=0.2.175"`, already pinned in `desktop/src-tauri/Cargo.toml`.
- Produces: `ProcessControl::{new,cancel,is_cancelled,pause,resume,prepare_command,register_child,clear_child}`.
- Produces: `CancellationToken` as a compatibility alias or wrapper around `ProcessControl`, so existing executor traits retain their signatures.

- [ ] **Step 1: Write failing process-control tests**

Create macOS/Unix tests in `process_control.rs` using a shell child that appends to a heartbeat file every 50 ms. Assert:

```rust
control.prepare_command(&mut command);
let mut child = command.spawn().unwrap();
control.register_child(&mut child).unwrap();
wait_for_heartbeat_growth(&heartbeat);
control.pause().unwrap();
assert_heartbeat_stops(&heartbeat);
control.resume().unwrap();
assert_heartbeat_grows(&heartbeat);
control.cancel();
terminate_and_wait(&mut child);
```

Add a second test that calls `pause()` before registration and verifies a subsequently registered child immediately stops.

- [ ] **Step 2: Run the focused test and verify failure**

Run:

```bash
cd desktop/src-tauri
cargo test process_control --lib
```

Expected: module and controller are missing.

- [ ] **Step 3: Implement process groups and signals**

Use `std::os::unix::process::CommandExt::process_group(0)` before spawn. Store the active process-group ID behind a mutex and pause/cancel flags in atomics. Signal the negative PGID so descendants are controlled:

```rust
fn signal_group(pgid: i32, signal: i32) -> Result<(), AppError> {
    let result = unsafe { libc::kill(-pgid, signal) };
    if result == 0 { Ok(()) } else {
        Err(AppError::with_cause(
            "MEDIA_JOB_SIGNAL_FAILED",
            "无法控制媒体处理进程",
            std::io::Error::last_os_error().to_string(),
        ))
    }
}
```

`register_child` records `child.id()` as PGID and sends `SIGSTOP` immediately if the paused flag is already set. Ignore `ESRCH` only while clearing or terminating a child that has already exited; user-requested pause/resume must report it.

- [ ] **Step 4: Route every media child through the controller**

Before spawning FFmpeg and AI worker commands, call `prepare_command`; immediately after spawn call `register_child`; call `clear_child(child.id())` on every normal, error, and cancellation path. Existing cancellation polling remains and terminates the group before waiting.

Ensure ffprobe calls used inside an active job also use the controller or complete before the task is marked running; no unregistered long-running subprocess may bypass pause.

- [ ] **Step 5: Run process and executor regression tests**

Run:

```bash
cd desktop/src-tauri
cargo test process_control --lib
cargo test media::merge --lib
cargo test media::ai --lib
```

Expected: heartbeat pause/resume tests and existing media tests pass.

- [ ] **Step 6: Commit process control**

```bash
git add desktop/src-tauri/src/media/process_control.rs desktop/src-tauri/src/media/mod.rs desktop/src-tauri/src/media/merge.rs desktop/src-tauri/src/media/ai.rs
git commit -m "feat: control media worker processes"
```

### Task 3: Service lifecycle commands and merge output guard

**Files:**
- Modify: `desktop/src-tauri/src/media/mod.rs`
- Modify: `desktop/src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: state transitions and `ProcessControl` from Tasks 1–2.
- Produces: `MediaJobService::{pause,resume,delete,has_merged_video}`.
- Produces Tauri commands: `pause_media_job`, `resume_media_job`, `delete_media_job`, `has_merged_video`.

- [ ] **Step 1: Write failing service tests**

Add tests in `media/mod.rs` for:

```rust
let queued = service.start_merge(request)?;
assert_eq!(service.pause(&queued.id)?.status, MediaJobStatus::Paused);
assert_eq!(service.resume(&queued.id)?.status, MediaJobStatus::Queued);

service.delete(&queued.id)?;
assert_eq!(service.manager().job(&queued.id).unwrap_err().code, "MEDIA_JOB_NOT_FOUND");
```

Use a blocking fake executor to verify an active job pauses through its controller, resume restores `Running`, delete cancels it, record removal happens after worker exit, and the next queued job cannot start before that exit.

Create filesystem cases proving `合并视频/existing.MP4` returns true and `start_merge` returns `MERGE_OUTPUT_EXISTS`; symlink files, directories named `.mp4`, nested files, and non-MP4 files return false.

- [ ] **Step 2: Run focused service tests and verify failure**

Run:

```bash
cd desktop/src-tauri
cargo test media::tests --lib
```

Expected: lifecycle and guard methods are missing.

- [ ] **Step 3: Implement pause and resume coordination**

For queued jobs, call the queued manager transition directly. For a running job, obtain the matching controller from `running`, signal it first, then persist `Paused + Running`; reverse the order safely on resume and roll back the signal if persistence fails.

Emit the returned job after each successful state change. Wake the worker only when resuming a queued-origin job.

- [ ] **Step 4: Implement deferred deletion**

Add `pending_removals: Arc<Mutex<HashSet<String>>>`. For non-running jobs, remove immediately. For active or running-origin-paused jobs, insert the ID, resume if paused, cancel the process group, and let `worker_loop` remove the record after executor return instead of applying `Cancel` or `Fail`.

The delete command returns after pending deletion has been registered and termination signalled. If signalling fails, remove the pending flag, keep the record, and return `MEDIA_JOB_SIGNAL_FAILED`. Suppress progress and terminal events for IDs in `pending_removals`.

- [ ] **Step 5: Implement filesystem-authoritative merge guard**

Add a helper with exact behavior:

```rust
pub fn has_merged_video(&self, series_root: &Path) -> Result<bool, AppError>
```

Canonicalize the series root, inspect `symlink_metadata(root.join("合并视频"))`, reject/fall back to false for a missing directory, reject a symlink directory, and check only direct entries whose `symlink_metadata` is a regular file and extension equals `mp4` ignoring ASCII case.

Serialize guard-check and `enqueue_merge` with a service mutex. Reject all new `Overwrite` requests and force the validated request to `FailIfExists`. Apply the same guard during retry of merge jobs.

- [ ] **Step 6: Register Tauri commands**

Add synchronous commands returning `AppResult<MediaJob>` for pause/resume, `AppResult<()>` for delete, and `AppResult<bool>` for the merge query. Register all four in `tauri::generate_handler!`.

- [ ] **Step 7: Run service and command tests**

Run:

```bash
cd desktop/src-tauri
cargo test media --lib
cargo check
```

Expected: all media tests pass and Tauri command registration compiles.

- [ ] **Step 8: Commit service behavior**

```bash
git add desktop/src-tauri/src/media/mod.rs desktop/src-tauri/src/lib.rs
git commit -m "feat: manage media job lifecycle"
```

### Task 4: React controls and merge-state UI

**Files:**
- Modify: `desktop/src/media/types.ts`
- Modify: `desktop/src/media/commands.ts`
- Modify: `desktop/src/media/useMediaJobs.ts`
- Modify: `desktop/src/media/useMediaJobs.test.tsx`
- Modify: `desktop/src/components/MediaJobsPanel.tsx`
- Modify: `desktop/src/components/DownloadManagerPage.tsx`
- Modify: `desktop/src/components/DownloadManagerPage.test.tsx`
- Modify: `desktop/src/components/MergeVideoDialog.tsx`
- Modify: `desktop/src/components/MergeVideoDialog.test.tsx`

**Interfaces:**
- Consumes Tauri commands from Task 3.
- Produces `MediaJobsModel::{pause,resume,deleteJob,hasMergedVideo}` and `MediaJobStatus` containing `paused`.
- Produces a per-batch merged-state map keyed by canonical series-root input string.

- [ ] **Step 1: Write failing hook tests**

Extend the fake `MediaCommands` and assert:

```ts
await result.current.pause("job-1");
expect(commands.pause).toHaveBeenCalledWith("job-1");
expect(result.current.jobs[0].status).toBe("paused");

await result.current.deleteJob("job-1");
expect(result.current.jobs).toEqual([]);

await expect(result.current.hasMergedVideo("/series")).resolves.toBe(true);
```

Also assert failed commands restore the prior job list and expose the normalized stable-code error.

- [ ] **Step 2: Write failing component tests**

In `DownloadManagerPage.test.tsx`, return true from `hasMergedVideo` and verify the completed batch button is disabled with text/title `已合并`. Return false and verify it opens the dialog. In panel tests, verify queued/running show pause, paused shows continue, all statuses show delete, and failed/interrupted/cancelled still show retry.

Update `MergeVideoDialog.test.tsx` to assert there is no overwrite radio and submitted options always contain `conflictPolicy: "failIfExists"`.

- [ ] **Step 3: Run frontend tests and verify failure**

Run:

```bash
cd desktop
npm test -- --run src/media/useMediaJobs.test.tsx src/components/MergeVideoDialog.test.tsx src/components/DownloadManagerPage.test.tsx
```

Expected: missing methods, statuses, and controls fail.

- [ ] **Step 4: Implement TypeScript command and hook APIs**

Extend `MediaCommands` with:

```ts
pause(jobId: string): Promise<MediaJob>;
resume(jobId: string): Promise<MediaJob>;
deleteJob(jobId: string): Promise<void>;
hasMergedVideo(seriesRoot: string): Promise<boolean>;
```

Upsert pause/resume results. Delete optimistically but keep the previous array and restore it on rejection. Normalize `MERGE_OUTPUT_EXISTS` as `已存在合并视频，请先移走或删除后再合并`.

- [ ] **Step 5: Implement media task controls**

Add `paused` to status labels and active filtering. Replace the running-only cancel button with pause/continue actions and add delete to every task. While deletion is pending, label the button `正在停止…` and disable all actions for that row.

- [ ] **Step 6: Implement the merge-state query in the download page**

Derive completed batch series roots, query each unique root after snapshot/job updates, and store results with an active-effect guard. Disable the merge button when true, render `已合并`, and do not open the dialog. Refresh after merge completion and after media task deletion.

Remove the overwrite fieldset from `MergeVideoDialog`; submit `failIfExists` unconditionally.

- [ ] **Step 7: Run frontend tests and typecheck**

Run:

```bash
cd desktop
npm test -- --run src/media/useMediaJobs.test.tsx src/components/MergeVideoDialog.test.tsx src/components/DownloadManagerPage.test.tsx
npm run build
```

Expected: focused tests pass and TypeScript/Vite build succeeds.

- [ ] **Step 8: Commit the frontend controls**

```bash
git add desktop/src/media/types.ts desktop/src/media/commands.ts desktop/src/media/useMediaJobs.ts desktop/src/media/useMediaJobs.test.tsx desktop/src/components/MediaJobsPanel.tsx desktop/src/components/DownloadManagerPage.tsx desktop/src/components/DownloadManagerPage.test.tsx desktop/src/components/MergeVideoDialog.tsx desktop/src/components/MergeVideoDialog.test.tsx
git commit -m "feat: add media job controls"
```

### Task 5: Full verification and release package

**Files:**
- Modify only if needed for release version: `desktop/src-tauri/tauri.conf.json`
- Modify only if needed for release version: `desktop/package.json`
- Modify only if needed for release version: `desktop/package-lock.json`
- Modify only if needed for checksums: `SHA256SUMS`

**Interfaces:**
- Consumes all Tasks 1–4.
- Produces a signed-or-unsigned local ARM64 DMG matching the project’s existing packaging flow and a SHA-256 digest.

- [ ] **Step 1: Run the full regression suite once**

Run:

```bash
python3 -m pytest
cd desktop && npm test -- --run
cd src-tauri && cargo test --lib
```

Expected: all existing Python, frontend, and Rust tests pass. Investigate failures against the dirty-worktree baseline before changing unrelated code.

- [ ] **Step 2: Run release builds**

Run:

```bash
cd desktop
npm run build
npm run tauri build
```

Expected: frontend build and macOS ARM64 application/DMG bundling succeed.

- [ ] **Step 3: Inspect the packaged application**

Verify the bundle contains the current API sidecar, FFmpeg/ffprobe resources, AI manifest, and expected app version. Mount or inspect the DMG and ensure the application launches without requesting a fresh runtime download when the already-installed runtime passes integrity checks.

- [ ] **Step 4: Compute and record the DMG checksum**

Run:

```bash
shasum -a 256 desktop/src-tauri/target/release/bundle/dmg/*.dmg
```

Record the exact filename and checksum in `SHA256SUMS` using the repository’s existing format.

- [ ] **Step 5: Review the final diff and commit release metadata**

```bash
git diff --check
git status --short
git add desktop/src-tauri/tauri.conf.json desktop/package.json desktop/package-lock.json SHA256SUMS
git commit -m "chore: package media job controls"
```

Only stage release metadata files that actually changed. Confirm no `.playwright-cli`, model cache, runtime archive, user media, OAuth credential, or unrelated dirty file enters a commit.
