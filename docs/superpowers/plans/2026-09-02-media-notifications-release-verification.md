# Media Notifications, Release Packaging, and End-to-End Verification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish notification routing, application integration, release-resource auditing, packaged runtime checks, and end-to-end acceptance for downloads, new releases, media jobs, and YouTube uploads.

**Architecture:** A typed notification router adds safe task targets and handles plugin action callbacks to focus the corresponding page/job. The release pipeline composes the API sidecar and verified media tools, renders the optional AI component manifest, builds Tauri artifacts, scans them for prohibited data, and runs packaged smoke checks before any live user OAuth/upload validation.

**Tech Stack:** React 19, TypeScript, Vitest, Tauri notification plugin 2.3.3 (`onAction`/`extra`), Rust/Tauri 2.8, shell release scripts, macOS `.app`/DMG, FFmpeg/ffprobe.

**Spec:** `docs/superpowers/specs/2026-09-02-media-processing-youtube-upload-design.md`

## Global Constraints

- Requires completion of the media foundation/merge, AI audio/subtitle, and YouTube plans for full acceptance. Steps may be run incrementally, but results must identify unavailable prior phases.
- System notifications are never the only result channel; foreground/in-app job state remains authoritative.
- Notify once per drama download batch, new-release refresh group, terminal media job, or terminal YouTube upload; never notify once per processed episode.
- Clicking a notification opens/focuses the app and selects the related monitor/batch/media/upload view without embedding filesystem paths or secrets in the notification.
- Notifications may include drama title, episode count, and safe status only. No credentials, token values, upload session URLs, device IDs, signatures, cookies, or raw error details.
- Notification denial must not trigger repeated permission prompts; Settings shows the denied state and system-settings guidance.
- Base `.app`/DMG includes the API sidecar, FFmpeg, ffprobe, required license notices, and the non-secret AI component manifest; it excludes AI runtime/model payloads and every OAuth credential/token.
- Release scripts fail closed on missing/mismatched tool checksums or missing generated sidecars.
- Do not claim a package is self-contained until it launches and performs health/merge checks with a minimal `PATH`.
- Do not claim real OAuth/upload until the user performs Google login/consent and a disposable upload.
- Code examples' test fixtures/builders must be defined in the corresponding test file and task; they are not hidden interfaces available from the existing project.
- Current workspace is not a Git repository; checkpoints record evidence without Git actions.

## File Structure

- Modify `desktop/src/notifications.ts`, `.test.ts`: typed notification payloads, permission cache, action listener.
- Create `desktop/src/notifications/useNotificationRouter.ts`, `.test.tsx`: notification target routing and terminal dedupe.
- Modify `desktop/src/settings/SettingsPage.tsx`, `useAppSettings.ts`, tests, `desktop/src/types.ts`, native settings: media/upload notification toggles.
- Modify `desktop/src/App.tsx`, `App.test.tsx`, `preview.ts`: route action targets and integrate every job model.
- Modify `desktop/src/components/DownloadManagerPage.tsx`, tests, `styles.css`: focused task and final three-tab states.
- Create `scripts/build-release.sh`: ordered release orchestrator.
- Create `scripts/verify-release.sh`: architecture, sidecar/tool, dependency, secret, license, and artifact checks.
- Create `THIRD_PARTY_NOTICES.md`: FFmpeg/ffprobe and packaged component notices.
- Modify `desktop/src-tauri/tauri.conf.json`: bundled notices/resources.
- Modify `desktop/README.md`, root `README.md`: setup, AI download, OAuth, copyright, and validation boundaries.

---

### Task 1: Typed Notification Payloads and Click Routing

**Files:**
- Modify: `desktop/src/notifications.ts`
- Modify: `desktop/src/notifications.test.ts`
- Create: `desktop/src/notifications/useNotificationRouter.ts`
- Create: `desktop/src/notifications/useNotificationRouter.test.tsx`
- Modify: `desktop/src/App.tsx`

**Interfaces:**
- Produces: `NotificationTarget = monitor | downloadBatch | mediaJob | youtubeJob`.
- Produces: `NotificationAdapter.send(message)` and `subscribeActions(listener)`.
- Produces: `useNotificationRouter({navigate,focusBatch,focusJob})`.

- [ ] **Step 1: Write failing permission, payload, and action tests**

```ts
it("routes a media notification action without exposing its path", async () => {
  const runtime = fakeRuntime();
  const adapter = createNotificationAdapter(runtime);
  await adapter.send({
    title: "字幕提取完成",
    body: "《测试剧》已完成",
    target: { kind: "mediaJob", id: "job-1" },
  });
  expect(runtime.lastOptions().extra).toEqual({ targetKind: "mediaJob", targetId: "job-1" });
  expect(JSON.stringify(runtime.lastOptions())).not.toContain("/Users/");
});
```

Test one permission request, denied cache, send failure fallback, `onAction` cleanup, invalid target rejection, and navigation to each target kind.

- [ ] **Step 2: Run and verify RED**

Run: `cd desktop && npm test -- src/notifications.test.ts src/notifications/useNotificationRouter.test.tsx`

Expected: failures because the current adapter accepts only title/body and has no action subscription.

- [ ] **Step 3: Implement the typed adapter**

```ts
export type NotificationMessage = {
  title: string;
  body: string;
  target: { kind: "monitor" | "downloadBatch" | "mediaJob" | "youtubeJob"; id: string };
};
```

Use plugin `extra` for target kind/ID and `onAction()` for click events. Accept only known kind plus a bounded opaque ID. Do not put output path, error message, OAuth/account details, or URL into `extra`.

- [ ] **Step 4: Implement action routing**

On action, focus the main window, set `nav` to monitor or queue, select the proper download-manager tab, and set the focused batch/job ID. If the record no longer exists, open the relevant page and show `任务记录已不存在` without recreating it.

- [ ] **Step 5: Migrate existing callers and run tests**

Update download completion and new-release callers to the typed message form. Run: `cd desktop && npm test -- src/notifications src/monitor src/download/useDownloadManager.test.tsx src/App.test.tsx`

Expected: PASS and only one action listener is active.

---

### Task 2: Media/Upload Notification Settings and Exactly-Once Terminal Events

**Files:**
- Modify: `desktop/src-tauri/src/settings.rs`
- Modify: `desktop/src/types.ts`
- Modify: `desktop/src/settings/useAppSettings.ts`
- Modify: `desktop/src/settings/useAppSettings.test.tsx`
- Modify: `desktop/src/settings/SettingsPage.tsx`
- Modify: `desktop/src/settings/SettingsPage.test.tsx`
- Modify: `desktop/src/notifications/useNotificationRouter.ts`
- Modify: `desktop/src/notifications/useNotificationRouter.test.tsx`
- Modify: `desktop/src/media/useMediaJobs.ts`

**Interfaces:**
- Adds settings `notifyMediaComplete: boolean` and `notifyYouTubeResult: boolean`, default true.
- Uses persisted per-job `completionNotifiedAt`/`failureNotifiedAt` claims to avoid restart duplicates.

- [ ] **Step 1: Write migration and exactly-once tests**

```tsx
it("aggregates a multi-episode subtitle job into one notification", async () => {
  const router = renderRouter({ notifyMediaComplete: true });
  router.completeJob(subtitleJobWithTwentyOutputs);
  await waitFor(() => expect(notifications.send).toHaveBeenCalledTimes(1));
  expect(notifications.send).toHaveBeenCalledWith(expect.objectContaining({
    title: "字幕提取完成",
    body: expect.stringContaining("20 集"),
  }));
});
```

Test restart with notification marker, media failure, YouTube success, YouTube failure, thumbnail partial success, toggles off, and permission denied.

- [ ] **Step 2: Implement settings migration**

Advance the native settings schema from the AI plan's version to the next integer and migrate absent toggles to true. Keep all download/model fields unchanged. Invalid values fall back with a non-blocking warning.

- [ ] **Step 3: Claim terminal notifications in persistent state**

The job store, not transient React state, owns notification claim timestamps. Add a native `mark_media_job_notified(job_id, outcome)` command that atomically persists the marker; the router calls it only after attempting the OS notification, while in-app state always updates regardless of permission.

- [ ] **Step 4: Add Settings switches and copy**

Add `媒体处理结果通知` and `YouTube 上传结果通知`. Preserve existing download/new-release toggles and denied-permission guidance.

- [ ] **Step 5: Run tests/checkpoint**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml settings:: media::storage && cd desktop && npm test -- src/settings src/notifications src/media/useMediaJobs.test.tsx`

Expected: PASS with no duplicate terminal notifications.

---

### Task 3: Final App Composition and Download Manager Focus Behavior

**Files:**
- Modify: `desktop/src/App.tsx`
- Modify: `desktop/src/App.test.tsx`
- Modify: `desktop/src/components/DownloadManagerPage.tsx`
- Modify: `desktop/src/components/DownloadManagerPage.test.tsx`
- Modify: `desktop/src/preview.ts`
- Modify: `desktop/src/preview.test.ts`
- Modify: `desktop/src/styles.css`

**Interfaces:**
- `DownloadManagerPage` final props include download manager, media jobs, YouTube model, active tab, focused batch/job, and focus callbacks.
- Four independent batch buttons remain directly accessible on completed batches.

- [ ] **Step 1: Write final composition tests**

```tsx
it("keeps four independent completed-batch actions", () => {
  renderCompletedBatch();
  for (const name of ["合并视频", "分离背景音乐", "提取字幕", "上传 YouTube"]) {
    expect(screen.getByRole("button", { name })).toBeEnabled();
  }
});
```

Test upload disabled reasons (no merge/unauthorized), notification focus, active tab persistence in session, focused row scroll, stale focus, preview isolation, and narrow-window dialogs.

- [ ] **Step 2: Run and verify RED**

Run: `cd desktop && npm test -- src/App.test.tsx src/components/DownloadManagerPage.test.tsx`

Expected: failures until every plan's model is composed.

- [ ] **Step 3: Refactor composition without expanding domain logic in App.tsx**

Create small composition hooks/components if `App.tsx` would otherwise own job rules. `App.tsx` may select adapters, route pages, coordinate notification focus, and show toasts; it must not reimplement native job transitions, OAuth, or model-install logic.

- [ ] **Step 4: Finalize three-tab manager UX**

Ensure download, media, and YouTube tabs show counts/badges and safe empty states. Focus from a notification selects the right tab and row. All progress bars have accessible labels; error details are expandable and already redacted.

- [ ] **Step 5: Add deterministic preview matrix**

Preview data includes incomplete download, completed batch, queued/running/failed/completed merge, separation, subtitle, upload, forced-private result, and thumbnail partial success. Preview never starts timers, downloads, model installs, OAuth, subprocesses, or network calls.

- [ ] **Step 6: Run frontend suite/build and visual check**

Run: `cd desktop && npm test && npm run build`

Expected: PASS. Check 1440×900, 1180×760, and one narrower test viewport for clipped controls, modal overflow, five-column library regressions, and inaccessible task actions.

---

### Task 4: Release Notices and Fail-Closed Build Orchestration

**Files:**
- Create: `scripts/build-release.sh`
- Create: `scripts/verify-release.sh`
- Create: `THIRD_PARTY_NOTICES.md`
- Modify: `desktop/src-tauri/tauri.conf.json`
- Modify: `desktop/README.md`
- Modify: `README.md`

**Interfaces:**
- `build-release.sh` orchestrates sidecar build, media-tool staging, AI manifest rendering, tests, and `npm run tauri build`.
- `verify-release.sh <app-path> <dmg-path>` exits non-zero on missing/wrong-arch resources, secret fixtures, AI payloads, or missing notices.

- [ ] **Step 1: Write shell-level failure tests**

Use a temporary fake bundle and assert verification fails for each condition: missing API sidecar, missing ffprobe, x86_64-only binary, non-executable tool, absent notice, bundled OAuth JSON, synthetic secret value, refresh-token fixture, AI model weight extension, and sidecar health failure.

- [ ] **Step 2: Implement ordered release orchestration**

```bash
set -euo pipefail
./scripts/build-api-sidecar.sh
./scripts/stage-media-tools.sh
./scripts/render-ai-component-manifest.sh
python3 -m unittest discover -s tests -v
(cd desktop && npm test && npm run build)
cargo test --manifest-path desktop/src-tauri/Cargo.toml
(cd desktop && npm run tauri build)
```

The script must print artifact paths/checksums only, never environment values. Missing required release variables fail before changing staged binaries.

- [ ] **Step 3: Implement bundle verification**

Use `file`, `lipo -info`, `codesign --verify --deep --strict` when applicable, direct `--version`/`--health-probe`, `find`, and `shasum`. Scan for the complete known synthetic test secrets and prohibited credential/model filenames. Verify AI manifest URLs are HTTPS and hashes are 64 hex characters.

- [ ] **Step 4: Add license and user-boundary documentation**

Document FFmpeg build/license configuration, included third-party notices, AI model first-use size/location/delete behavior, copyright disclaimer, Google Cloud prerequisite, system-browser authorization, public-vs-forced-private warning, Keychain storage, credential removal, and the fact that real consent/upload must be user-operated.

- [ ] **Step 5: Run release-script tests**

Expected: every intentionally invalid fixture fails with a specific message; a valid synthetic bundle passes. No destructive cleanup targets broad directories.

---

### Task 5: Full Automated Regression and Packaged Runtime Proof

**Files:**
- Modify only files already named by the four plans when a failing check requires a fix.

**Interfaces:**
- Validates the complete non-live application claim.

- [ ] **Step 1: Run every automatic suite from a clean process state**

```bash
python3 -m unittest discover -s tests -v
python3 -m unittest discover -s ai_worker/tests -v
cd desktop && npm test && npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: all exit 0. Record exact test counts.

- [ ] **Step 2: Run browser regression**

Verify homepage/search/rank remain five columns and 20-item groups, new-release monitoring still filters Shanghai-today selected types, settings persist, download queue still defaults to 5 and allows 1–10, and every new modal/task tab is usable.

- [ ] **Step 3: Build and verify release artifacts**

Run `scripts/build-release.sh`, then `scripts/verify-release.sh` against the generated `.app` and `.dmg`. Record absolute artifact paths and SHA-256.

- [ ] **Step 4: Prove packaged self-containment**

Launch the packaged `.app` with a minimal `PATH`. Verify API health, discovery request, download queue visibility, bundled tool status, one generated-fixture merge, settings restart persistence, system notification, notification click focus, and no system Python/FFmpeg lookup.

- [ ] **Step 5: Exercise AI first-use boundaries**

With no AI component installed, confirm ordinary download/merge works and AI actions present install confirmation. With a local test manifest server, verify install, checksum, self-test, separation, subtitle, delete, and re-download. Any real remote model download remains separately identified.

- [ ] **Step 6: Exercise mocked YouTube boundaries**

Run the packaged app against test-only injected endpoints in a test build, not production endpoint overrides. Verify safe credential rejection, authorization callback simulation, progress/cancel, 401 refresh, 308 resume, 404 expiry, actual privacy rendering, and thumbnail partial success/retry.

- [ ] **Step 7: Record non-live completion evidence**

Report which automated, browser, native, packaged, AI-runtime, and mocked-YouTube layers passed. List Windows, Intel, signing/notarization, remote model hosting, real Google consent, real upload, and Content ID behavior as unverified unless direct evidence exists.

---

### Task 6: User-Operated Live OAuth and Disposable Upload Acceptance

**Files:**
- No source files should change solely to make a live account test pass. Any discovered bug returns to the relevant earlier task with a regression test first.

**Interfaces:**
- Validates the live account layer only after non-live acceptance is complete.

- [ ] **Step 1: Hand the app to the user at credential selection**

The user selects their Desktop OAuth JSON. Confirm only configured status and safe channel summary appear. Do not print, inspect, paste, or store the credential outside the app-private copy.

- [ ] **Step 2: User completes system-browser consent**

The user chooses the Google account/channel and approves the `youtube.upload` scope. Observe callback success and Keychain-backed restart persistence without taking over the browser.

- [ ] **Step 3: User selects a disposable merged test video**

Confirm inherited title/description/cover, default no-music source when present, public request, not-made-for-kids value, synthetic-content declaration, and final publish confirmation. The user decides whether to continue.

- [ ] **Step 4: Observe real upload result**

Record progress, video ID/link, server actual privacy, thumbnail result, and YouTube processing status. If the project is forced private, report the actual private result as correct behavior.

- [ ] **Step 5: Optionally test an interruption only when safe**

If the user approves, interrupt a disposable upload and confirm server-offset resume. Do not risk an important video or channel state merely to exercise this path; mocked 308 coverage remains the primary proof otherwise.

- [ ] **Step 6: Final handoff**

Provide artifact paths/checksums, exact validation matrix, privacy/thumbnail result, remaining boundaries, AI/copyright disclaimer, and recovery instructions. Never state that music removal guarantees avoiding Content ID or copyright claims.
