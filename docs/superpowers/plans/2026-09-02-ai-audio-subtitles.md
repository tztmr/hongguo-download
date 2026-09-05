# AI Audio Separation and Subtitle Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add on-demand AI runtime/model management, independent Demucs background-music separation, and independent subtitle extraction/Whisper transcription for episode or merged-video scope.

**Architecture:** Rust extends the native media-job manager from Plan 1 with a checksum-verified component installer and a JSON-lines AI worker protocol. A separate Python worker owns Demucs/Whisper calls while FFmpeg remains responsible for audio extraction/remuxing; React exposes model settings and two separate task dialogs.

**Tech Stack:** Tauri 2.8, Rust 2021, reqwest, sha2, tar/zip extraction, Python 3.11 worker, PyTorch, Demucs `htdemucs`, OpenAI Whisper `small`, FFmpeg/ffprobe, React 19, TypeScript, Vitest.

**Spec:** `docs/superpowers/specs/2026-09-02-media-processing-youtube-upload-design.md`

## Global Constraints

- Requires completion of `docs/superpowers/plans/2026-09-02-media-job-foundation-merge.md`.
- `分离背景音乐`和`提取字幕`是两个独立按钮、命令、任务类型和重试单元，禁止合并成一个工作流。
- 已有合并视频时，两个功能都必须让用户选择逐集或合并视频；无合并视频时只允许逐集。
- Demucs 默认 `htdemucs`；Whisper 默认 `small`；设置中可选已支持模型。
- AI 运行环境和模型首次使用时按需下载，不进入基础 DMG。
- 安装必须预检查空间、下载到临时文件、校验 SHA-256，成功后原子替换。
- 字幕先导出可转换的文本字幕轨；否则使用 Whisper。不对烧录字幕做 OCR。
- 音乐分离失败不得阻止字幕提取；无人声轨时 Whisper 使用原音频。
- 人声、背景音乐、去背景音乐视频、SRT 都不覆盖原始下载。
- UI 必须明确：AI 音源分离可能残留/失真，不保证规避 Content ID 或版权责任。
- 代码示例中的测试夹具/构造器必须在对应测试文件和任务内一并定义；它们不是可从现有项目导入的隐藏接口。
- 当前工作区不是 Git 仓库；检查点只记录差异和验证证据。

## File Structure

### Native component manager

- Create `desktop/src-tauri/src/media/components.rs`: release manifest, status, download, checksum, install, delete.
- Create `desktop/src-tauri/resources/ai-components.schema.json`: non-secret manifest schema checked at build/test time.
- Create `scripts/render-ai-component-manifest.sh`: require release URLs/checksums and emit the bundled manifest.
- Modify `desktop/src-tauri/src/media/model.rs`, `media/mod.rs`, `lib.rs`, `Cargo.toml`, `tauri.conf.json`.

### Python AI worker

- Create `ai_worker/main.py`: one-request JSON-lines CLI entrypoint.
- Create `ai_worker/protocol.py`: request/event/result validation and safe errors.
- Create `ai_worker/separate.py`: Demucs two-stem separation.
- Create `ai_worker/transcribe.py`: Whisper SRT generation.
- Create `ai_worker/tests/test_protocol.py`, `test_separate.py`, `test_transcribe.py`.
- Create `requirements-ai.txt`: runtime dependency inputs for the versioned AI artifact build.
- Create `scripts/build-ai-runtime.sh`: build relocatable ARM64 worker runtime and model-independent self-test.

### React

- Modify `desktop/src/types.ts`, `desktop/src/api.ts`: settings/model status contracts.
- Modify `desktop/src/settings/useAppSettings.ts`, `SettingsPage.tsx`, and tests: model choices and component manager.
- Modify `desktop/src/media/types.ts`, `commands.ts`, `useMediaJobs.ts`, and tests: two new job types.
- Create `desktop/src/components/MediaScopeDialog.tsx` and `.test.tsx`.
- Modify `desktop/src/components/DownloadManagerPage.tsx` and `.test.tsx`: two independent buttons/job filters.
- Modify `desktop/src/App.tsx`, `preview.ts`, `styles.css`.

---

### Task 1: Versioned AI Component Manifest and Atomic Installer

**Files:**
- Create: `desktop/src-tauri/src/media/components.rs`
- Create: `desktop/src-tauri/resources/ai-components.schema.json`
- Create: `scripts/render-ai-component-manifest.sh`
- Modify: `desktop/src-tauri/src/media/mod.rs`
- Modify: `desktop/src-tauri/src/lib.rs`
- Modify: `desktop/src-tauri/Cargo.toml`
- Modify: `desktop/src-tauri/tauri.conf.json`

**Interfaces:**
- Produces: `ComponentManager::status()`, `install(component_id, progress)`, `remove(component_id)`.
- Produces commands/events: `get_ai_components`, `install_ai_component`, `remove_ai_component`, `ai-component-progress`.
- Manifest entries: `id`, `version`, `platform`, `url`, `sha256`, `downloadBytes`, `installedBytes`, `entrypoint`.

- [ ] **Step 1: Write failing manifest/checksum/atomicity tests**

```rust
#[test]
fn checksum_failure_never_replaces_the_current_component() {
    let fixture = component_fixture_with_existing_version("1");
    let error = fixture.install_bytes("runtime", b"corrupt", "00").unwrap_err();
    assert_eq!(error.code, "AI_COMPONENT_CHECKSUM_FAILED");
    assert_eq!(fixture.installed_version("runtime"), Some("1"));
}

#[test]
fn running_component_cannot_be_removed() {
    let fixture = component_fixture_marked_in_use();
    assert_eq!(fixture.remove("runtime").unwrap_err().code, "AI_COMPONENT_IN_USE");
}
```

Also test unsupported platform, insufficient disk, interrupted temp file, path traversal inside archives, and manifest schema mismatch.

- [ ] **Step 2: Run and verify RED**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml media::components`

Expected: compile failure for missing component module.

- [ ] **Step 3: Implement strict manifest and install state**

```rust
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentRelease {
    pub id: String,
    pub version: String,
    pub platform: String,
    pub url: Url,
    pub sha256: String,
    pub download_bytes: u64,
    pub installed_bytes: u64,
    pub entrypoint: String,
}
```

Allow HTTPS only. Validate lowercase 64-character SHA-256, expected platform `aarch64-apple-darwin`, safe relative entrypoint, and bounded sizes. Download to `<app-data>/components/.downloads/<id>.part`, check free space before network use, hash while streaming, extract into a random staging directory with path-containment checks, run `entrypoint --self-test`, then rename to `<id>/<version>` and update an atomic `installed.json`.

- [ ] **Step 4: Implement build-time manifest rendering**

The script requires explicit `HONGGUO_AI_RUNTIME_URL`, `HONGGUO_AI_RUNTIME_SHA256`, sizes, and model URL/checksum variables. Missing variables exit non-zero; do not commit blank URLs or zero checksums. The generated manifest is non-secret and bundled as a Tauri resource.

- [ ] **Step 5: Add native commands with single-install locking**

Reject concurrent installs with `AI_COMPONENT_INSTALL_RUNNING`. Progress stages are `checking`, `downloading`, `verifying`, `extracting`, `selfTesting`, `installed`, `failed`. Removal is allowed only when no media job references the component.

- [ ] **Step 6: Run tests and checkpoint**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml media::components && cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --all-targets -- -D warnings`

Expected: PASS; tests use a local mock server and synthetic archives only.

---

### Task 2: App Settings v2 and Model Management UI

**Files:**
- Modify: `desktop/src-tauri/src/settings.rs`
- Modify: `desktop/src/types.ts`
- Modify: `desktop/src/api.ts`
- Modify: `desktop/src/settings/useAppSettings.ts`
- Modify: `desktop/src/settings/useAppSettings.test.tsx`
- Modify: `desktop/src/settings/SettingsPage.tsx`
- Modify: `desktop/src/settings/SettingsPage.test.tsx`
- Modify: `desktop/src/styles.css`

**Interfaces:**
- Adds settings: `demucsModel: "htdemucs" | "htdemucs_ft"`, `whisperModel: "small" | "medium"`.
- Adds `AIComponentStatus` and install/remove operations to `UseAppSettingsResult` or a focused `useAIComponents` hook.
- Migrates native settings version 1 to 2 without changing save directory, resolution, or notification toggles.

- [ ] **Step 1: Write failing migration and UI tests**

```rust
#[test]
fn v1_settings_migrate_to_default_ai_models() {
    let restored = load_settings(&write_v1_settings(), default_dir());
    assert_eq!(restored.demucs_model, "htdemucs");
    assert_eq!(restored.whisper_model, "small");
}
```

```tsx
it("shows component progress and refuses model deletion while in use", async () => {
  render(<SettingsPage model={modelWithRunningAIJob} />);
  expect(screen.getByText("htdemucs")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "删除模型" })).toBeDisabled();
});
```

- [ ] **Step 2: Run and verify RED**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml settings:: && cd desktop && npm test -- src/settings`

Expected: failures for missing v2 fields/UI.

- [ ] **Step 3: Implement explicit v1 migration and model validation**

Accept only the supported enum values; invalid stored values fall back to `htdemucs`/`small` with a non-blocking warning. Saving one field must preserve all other settings.

- [ ] **Step 4: Implement model-management settings cards**

Render runtime/model version, installed size, path, progress, install, delete, and redownload. Starting a media action when absent opens one confirmation listing runtime plus selected model download/installed bytes; it must not silently begin a large download.

- [ ] **Step 5: Run settings tests and build**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml settings:: && cd desktop && npm test -- src/settings && npm run build`

Expected: PASS.

---

### Task 3: JSON-Lines AI Worker Protocol and Self-Test

**Files:**
- Create: `ai_worker/protocol.py`
- Create: `ai_worker/main.py`
- Create: `ai_worker/tests/test_protocol.py`
- Create: `requirements-ai.txt`
- Create: `scripts/build-ai-runtime.sh`

**Interfaces:**
- Consumes one JSON request on stdin: `{version, jobId, operation, inputPath, outputDir, options}`.
- Emits JSON lines: progress `{type:"progress", stage, percent}`, result `{type:"result", outputs}`, or safe error `{type:"error", code, message}`.
- Supports `--self-test` without downloading models or contacting the network.

- [ ] **Step 1: Write protocol validation tests**

```python
def test_request_rejects_unknown_operation_and_outside_output(tmp_path):
    with self.assertRaisesRegex(WorkerError, "AI_REQUEST_INVALID"):
        parse_request({"version": 1, "jobId": "j1", "operation": "shell",
                       "inputPath": str(tmp_path / "in.mp4"),
                       "outputDir": "/tmp/outside", "options": {}})

def test_errors_never_include_environment_or_command_line():
    event = safe_error(RuntimeError("token=synthetic-secret /Users/name/file"))
    self.assertEqual(event["code"], "AI_WORKER_FAILED")
    self.assertNotIn("synthetic-secret", event["message"])
```

- [ ] **Step 2: Run and verify RED**

Run: `python3 -m unittest discover -s ai_worker/tests -p 'test_protocol.py' -v`

Expected: import failure.

- [ ] **Step 3: Implement strict protocol and stdout discipline**

Only JSON events go to stdout; library logs go to stderr at warning level. Validate schema version 1, known operations `separate`/`transcribe`, existing regular input file, and canonical output containment. Convert uncaught exceptions to stable safe errors.

- [ ] **Step 4: Implement offline self-test and build script**

`--self-test` imports torch, demucs, and whisper, checks expected functions/classes, writes one protocol progress/result pair to a temp directory, and exits. It must not resolve/download model weights. The build script creates the relocatable runtime archive used by Task 1's manifest.

- [ ] **Step 5: Run protocol tests**

Run: `python3 -m unittest discover -s ai_worker/tests -p 'test_protocol.py' -v`

Expected: PASS and every stdout line parses as JSON.

---

### Task 4: Demucs Two-Stem Separation and No-Music Video

**Files:**
- Create: `ai_worker/separate.py`
- Create: `ai_worker/tests/test_separate.py`
- Modify: `ai_worker/main.py`
- Modify: `desktop/src-tauri/src/media/model.rs`
- Modify: `desktop/src-tauri/src/media/mod.rs`

**Interfaces:**
- Worker operation `separate` produces `vocalsPath`, `backgroundMusicPath`.
- Rust task `AudioSeparation` additionally produces `noBackgroundMusicVideoPath` through bundled FFmpeg.
- Tauri command: `start_audio_separation_job(request)`.

- [ ] **Step 1: Write failing worker tests with an injected separator**

```python
def test_separation_returns_both_stems_and_reports_monotonic_progress(self):
    events = []
    result = separate_audio(request, separator=fake_separator, emit=events.append)
    self.assertTrue(Path(result["vocalsPath"]).is_file())
    self.assertTrue(Path(result["backgroundMusicPath"]).is_file())
    self.assertEqual(sorted(e["percent"] for e in events), [e["percent"] for e in events])
```

Test missing stem, zero-length stem, duration mismatch, cancellation/worker termination, and unsupported model.

- [ ] **Step 2: Implement deterministic preprocessing and Demucs call**

Rust extracts WAV into the job temp directory. The worker runs two-stem vocals mode with the selected model. Device `auto` may use MPS only after a synthetic self-test succeeds; otherwise use CPU. Outputs must be WAV files under the requested output directory.

- [ ] **Step 3: Implement native job orchestration and remux**

Sort scope inputs, create one child subtask per episode or one merged input, stream JSON progress, then remux vocals with the original video stream. Validate both stems and the no-music video with ffprobe before publishing to `<series>/音频分离/`.

- [ ] **Step 4: Add failure isolation tests**

A failed episode marks the job failed with the episode index and keeps prior validated outputs visible but does not mark the overall job completed. Retry reprocesses only missing/failed outputs when input snapshots are unchanged.

- [ ] **Step 5: Run focused suites**

Run: `python3 -m unittest ai_worker.tests.test_separate -v && cargo test --manifest-path desktop/src-tauri/Cargo.toml audio_separation`

Expected: PASS with fake model tests; optional real-model quality smoke is separately labeled.

---

### Task 5: Text Subtitle Extraction and Whisper SRT Generation

**Files:**
- Create: `ai_worker/transcribe.py`
- Create: `ai_worker/tests/test_transcribe.py`
- Modify: `ai_worker/main.py`
- Modify: `desktop/src-tauri/src/media/tools.rs`
- Modify: `desktop/src-tauri/src/media/model.rs`
- Modify: `desktop/src-tauri/src/media/mod.rs`

**Interfaces:**
- Produces `SubtitleSource = EmbeddedText | WhisperOriginalAudio | WhisperVocals`.
- Tauri command: `start_subtitle_job(request)`.
- Worker operation `transcribe` produces `srtPath`, language, and segment count.

- [ ] **Step 1: Write failing source-selection and SRT tests**

```rust
#[test]
fn embedded_text_wins_and_burned_or_bitmap_subtitles_use_whisper() {
    assert_eq!(select_subtitle_source(&probe_with("subrip"), None), SubtitleSource::EmbeddedText);
    assert_eq!(select_subtitle_source(&probe_with("hdmv_pgs_subtitle"), None), SubtitleSource::WhisperOriginalAudio);
}
```

```python
def test_segments_render_monotonic_utf8_srt():
    text = segments_to_srt([{"start": 0.0, "end": 1.2, "text": "你好"}])
    self.assertEqual(text, "1\n00:00:00,000 --> 00:00:01,200\n你好\n")
```

- [ ] **Step 2: Implement embedded text-track export**

Use ffmpeg stream mapping for supported text codecs (`subrip`, `ass`, `ssa`, `webvtt`, `mov_text`) and convert to UTF-8 SRT. If only bitmap subtitles or no subtitle stream exist, continue to Whisper; do not add OCR libraries.

- [ ] **Step 3: Implement Whisper transcription**

Prefer the matching validated vocals stem when present and input-compatible; otherwise extract mono 16kHz WAV from the source. Use the selected model, `fp16=false` on CPU, and render segments with clamped non-overlapping timestamps. Reject empty/no-speech output with `SUBTITLE_NO_SPEECH` rather than writing a blank completed SRT.

- [ ] **Step 4: Implement per-episode/merged publishing**

Per-episode scope publishes one SRT per episode; merged scope publishes one SRT matching the merged video. Validate sequence numbers, `start < end`, monotonic order, UTF-8 decoding, and last timestamp not exceeding source duration by more than one second.

- [ ] **Step 5: Run focused suites**

Run: `python3 -m unittest ai_worker.tests.test_transcribe -v && cargo test --manifest-path desktop/src-tauri/Cargo.toml subtitle`

Expected: PASS for embedded, bitmap/no-track fallback, vocals preference, malformed segments, and no-speech behavior.

---

### Task 6: Independent Scope Dialogs and Media Job UI

**Files:**
- Modify: `desktop/src/media/types.ts`
- Modify: `desktop/src/media/commands.ts`
- Modify: `desktop/src/media/useMediaJobs.ts`
- Modify: `desktop/src/media/useMediaJobs.test.tsx`
- Create: `desktop/src/components/MediaScopeDialog.tsx`
- Create: `desktop/src/components/MediaScopeDialog.test.tsx`
- Modify: `desktop/src/components/DownloadManagerPage.tsx`
- Modify: `desktop/src/components/DownloadManagerPage.test.tsx`
- Modify: `desktop/src/App.tsx`
- Modify: `desktop/src/preview.ts`
- Modify: `desktop/src/styles.css`

**Interfaces:**
- `startAudioSeparation(batch, scope)` invokes only `start_audio_separation_job`.
- `startSubtitleExtraction(batch, scope)` invokes only `start_subtitle_job`.
- `MediaScopeDialog` takes `kind: "audioSeparation" | "subtitleExtraction"`, `hasMergedVideo`, and returns `"episodes" | "merged"`.

- [ ] **Step 1: Write the independence regression tests**

```tsx
it("routes the two buttons to different commands", () => {
  render(<DownloadManagerPage manager={downloads} media={media} {...paths} />);
  fireEvent.click(screen.getByRole("button", { name: "分离背景音乐" }));
  fireEvent.click(screen.getByRole("button", { name: "开始分离" }));
  expect(media.startAudioSeparation).toHaveBeenCalledTimes(1);
  expect(media.startSubtitleExtraction).not.toHaveBeenCalled();
});
```

Add the inverse subtitle test. Test merged choice appears only when a validated merged output exists; otherwise the dialog states that per-episode scope is fixed.

- [ ] **Step 2: Run and verify RED**

Run: `cd desktop && npm test -- src/components/MediaScopeDialog.test.tsx src/components/DownloadManagerPage.test.tsx`

Expected: failures for missing dialog/buttons.

- [ ] **Step 3: Add independent buttons, copy, and output actions**

Render separate task kinds, output groups, locate actions, and retry buttons. Music rows list vocals/background/no-music video; subtitle rows list SRT/source. Show the copyright-risk disclaimer in the separation confirmation and result panel.

- [ ] **Step 4: Connect install preflight without coupling tasks**

Each action independently checks runtime plus its selected model. If absent, show the component confirmation, install, then enqueue only the originally requested job. Cancelling installation creates no media job.

- [ ] **Step 5: Run UI and build checks**

Run: `cd desktop && npm test -- src/media src/components/MediaScopeDialog.test.tsx src/components/DownloadManagerPage.test.tsx src/settings && npm run build`

Expected: PASS; accessibility queries find distinct button names and progress statuses.

- [ ] **Step 6: Browser checkpoint**

Use preview fixtures to verify per-episode vs merged dialogs, model missing/downloading/installed states, separate job filters, long Chinese output names, and narrow-window layout.

---

### Task 7: Plan-2 Regression and Optional Real-Model Smoke

**Files:**
- Modify only listed files when a failing acceptance check requires a fix.

**Interfaces:**
- Validates Plan 2 without changing Plan 1 merge semantics.

- [ ] **Step 1: Run all automatic tests**

```bash
python3 -m unittest discover -s ai_worker/tests -v
python3 -m unittest discover -s tests -v
cd desktop && npm test && npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: every command exits 0.

- [ ] **Step 2: Verify component failure modes with local fixtures**

Exercise interrupted download, wrong checksum, insufficient space, corrupt archive, failed self-test, delete, redownload, and in-use deletion. Expected: existing installed component remains usable or status is explicitly unavailable; no partial install is reported as ready.

- [ ] **Step 3: Run one user-approved real-model smoke on disposable media**

After the user approves the model download, process a short non-copyright fixture. Confirm three audio outputs, no-music video, and SRT are readable. Judge only runtime/file validity; label separation/transcription quality as a manual observation, not a correctness guarantee.

- [ ] **Step 4: Rebuild packaged app**

Run: `cd desktop && npm run tauri build`

Expected: base DMG still contains no AI runtime or model; after first-use install, components exist only under app data. Ordinary download and merge continue when AI components are removed.

- [ ] **Step 5: Record checkpoint**

Record test counts, component manifests/checksums used, installed disk usage, fixture paths, packaged artifact checksums, and any skipped real-model/network validation.
