# YouTube Authorization and Upload Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add secure Desktop OAuth import/authorization, Keychain token storage, merged-video-only resumable YouTube upload, inherited download metadata, and retryable custom-thumbnail publishing.

**Architecture:** Adapt the already validated Rust OAuth and resumable-upload modules from `/Users/edking/Documents/网赚学习/youtube上传视频` into the current native media-job manager. React owns editable upload intent and explicit confirmation; Rust owns credential validation/copying, system-browser PKCE, tokens, source snapshots, resumable checkpoints, server-truth results, and thumbnail publishing.

**Tech Stack:** Tauri 2.8, Rust 2021, tokio, reqwest/rustls, keyring, base64, sha2, atomicwrites, wiremock, React 19, TypeScript, Vitest, Testing Library, YouTube Data API v3.

**Spec:** `docs/superpowers/specs/2026-09-02-media-processing-youtube-upload-design.md`

## Global Constraints

- Requires completion of `2026-09-02-media-job-foundation-merge.md`; AI plan is optional for original-audio upload, but required for the default no-background-music source to be available.
- Only a validated merged video may be uploaded. Do not add per-episode batch upload.
- Default source is the no-background-music merged video when present; otherwise select original merged video and show the fallback explicitly.
- Title, description, and cover default to the immutable download batch snapshot; they remain editable before upload.
- Missing description falls back to the drama title only; never invent copy.
- Tags may derive from download type/category; SRT remains local and no caption API is called.
- Default privacy is `public`; default audience is not made for kids; AI drama defaults `containsSyntheticMedia=true`. Audience and synthetic declaration require explicit confirmation.
- Request only `https://www.googleapis.com/auth/youtube.upload`.
- OAuth login must use the system browser with loopback callback, PKCE S256, and random state; never use the app WebView for Google login.
- Imported credential JSON is copied into private app configuration with mode 0600. The original path, client ID value, client secret, access token, refresh token, authorization code, and upload-session URL must not appear in logs/UI/notifications.
- Refresh tokens live in macOS Keychain. Access tokens remain memory-only.
- Every upload starts only after explicit user confirmation; no processing completion may auto-publish.
- The UI displays the server-returned video ID, URL, and actual privacy state. It must warn that unverified API projects may be forced private.
- Thumbnail failure after video success is partial success and retries only the thumbnail; it must never re-upload the video.
- Google login/consent and the real upload are user-operated validation boundaries.
- Test fixtures/builders shown in examples must be defined in the corresponding test file and task; they are not hidden interfaces available from the existing project.
- Current workspace is not a Git repository; checkpoints record tests/diffs without Git actions.

## File Structure

### Rust

- Modify `desktop/src-tauri/Cargo.toml`, `src/lib.rs`, `capabilities/default.json`: async OAuth/upload dependencies and opener capability.
- Create `desktop/src-tauri/src/youtube/mod.rs`: module exports and YouTube channel client.
- Create `desktop/src-tauri/src/youtube/config.rs`: strict Desktop OAuth parser and private import.
- Create `desktop/src-tauri/src/youtube/vault.rs`: Keychain and memory test vault.
- Create `desktop/src-tauri/src/youtube/state.rs`: versioned authorized-channel state.
- Create `desktop/src-tauri/src/youtube/oauth.rs`: system-browser PKCE authorization/refresh/revoke.
- Create `desktop/src-tauri/src/youtube/models.rs`: public snapshots, upload request/result, thumbnail status.
- Create `desktop/src-tauri/src/youtube/upload.rs`: per-job resumable upload/checkpoint.
- Create `desktop/src-tauri/src/youtube/thumbnail.rs`: cover preparation and `thumbnails.set`.
- Create `desktop/src-tauri/src/youtube/commands.rs`: Tauri command boundary and safe progress emission.
- Modify `desktop/src-tauri/src/media/model.rs`, `storage.rs`, `mod.rs`: YouTube upload job kind and partial-success state.

### React

- Create `desktop/src/youtube/types.ts`, `commands.ts`, `useYouTube.ts`, `useYouTube.test.tsx`.
- Create `desktop/src/settings/YouTubeSettings.tsx` and `.test.tsx`.
- Create `desktop/src/components/YouTubeUploadDialog.tsx` and `.test.tsx`.
- Create `desktop/src/components/YouTubeUploadJobs.tsx` and `.test.tsx`.
- Modify `desktop/src/settings/SettingsPage.tsx`, `desktop/src/api.ts`, `desktop/src/types.ts`.
- Modify `desktop/src/components/DownloadManagerPage.tsx` and `.test.tsx`.
- Modify `desktop/src/App.tsx`, `desktop/src/preview.ts`, `desktop/src/styles.css`.

### Reuse references

- Read/adapt `youtube上传视频/src-tauri/src/oauth/config.rs`, `flow.rs`, `vault.rs`.
- Read/adapt `youtube上传视频/src-tauri/src/youtube/models.rs`, `upload.rs`, `commands.rs`.
- Preserve tested security and retry behavior, but do not copy its old keyring service name, single global checkpoint path, proxy UI, settings layout, or source credential path semantics.

---

### Task 1: Dependencies, Structured YouTube Types, and Safe Error Boundary

**Files:**
- Modify: `desktop/src-tauri/Cargo.toml`
- Modify: `desktop/src-tauri/src/lib.rs`
- Modify: `desktop/src-tauri/src/app_error.rs`
- Create: `desktop/src-tauri/src/youtube/mod.rs`
- Create: `desktop/src-tauri/src/youtube/models.rs`
- Modify: `desktop/src-tauri/capabilities/default.json`

**Interfaces:**
- Produces: `YouTubeSnapshot`, `CredentialSummary`, `AccountSummary`, `UploadIntent`, `UploadResult`, `ThumbnailState`.
- Extends `AppError` serialization to `{code,message}` while keeping internal causes non-serialized.
- Uses one reqwest version with async + blocking features to avoid duplicate HTTP stacks.

- [ ] **Step 1: Add failing serialization and validation tests**

```rust
#[test]
fn upload_intent_requires_audience_and_publish_confirmation() {
    let error = UploadIntent::fixture().without_audience_confirmation().validate().unwrap_err();
    assert_eq!(error.code, "UPLOAD_AUDIENCE_CONFIRMATION_REQUIRED");
}

#[test]
fn public_snapshot_never_serializes_token_or_session_fields() {
    let json = serde_json::to_string(&YouTubeSnapshot::fixture()).unwrap();
    assert!(!json.contains("refresh"));
    assert!(!json.contains("Bearer"));
    assert!(!json.contains("upload_session"));
}
```

- [ ] **Step 2: Add dependencies and module skeleton**

Use the current Tauri-compatible versions already proven by the uploader checkout where possible: `tokio`, `keyring` with Apple native support, `base64`, `sha2`, `subtle`, `atomicwrites`, `chrono`, `url`, `tauri-plugin-opener`; add `wiremock` and `tempfile` as dev dependencies. Expand the existing reqwest feature set to support both current blocking API/download calls and async form/json/query upload calls with rustls.

- [ ] **Step 3: Implement exact public models**

```rust
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadIntent {
    pub job_id: String,
    pub file_path: PathBuf,
    pub cover_path: Option<PathBuf>,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub category_id: String,
    pub privacy_status: PrivacyStatus,
    pub self_declared_made_for_kids: bool,
    pub contains_synthetic_media: bool,
    pub audience_confirmed: bool,
    pub synthetic_media_confirmed: bool,
    pub publish_confirmed: bool,
}
```

Validate non-empty title, supported privacy, category ID digits, at most the API-supported tag payload, all confirmations, regular source file, and merged-output provenance from the native media store.

- [ ] **Step 4: Run tests and checkpoint**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml youtube::models && cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --all-targets -- -D warnings`

Expected: PASS; no unused secret-bearing debug derives.

---

### Task 2: Strict Credential Import and Keychain Vault

**Files:**
- Create: `desktop/src-tauri/src/youtube/config.rs`
- Create: `desktop/src-tauri/src/youtube/vault.rs`
- Create: `desktop/src-tauri/src/youtube/state.rs`
- Modify: `desktop/src-tauri/src/youtube/mod.rs`

**Interfaces:**
- Produces: `OAuthClientConfig::from_json`, `import_private(source, app_config_dir) -> CredentialSummary`.
- Produces: `TokenVault` and `OsTokenVault` using service `com.edking.hongguo.desktop.youtube.refresh-token`.
- Produces: atomic `youtube-state.json` containing only channel summaries/active channel ID/credential configured flag.

- [ ] **Step 1: Port and strengthen sanitized config tests**

```rust
#[test]
fn import_copies_only_valid_desktop_google_credentials_with_mode_0600() {
    let source = write_synthetic_desktop_json();
    let imported = import_private(&source, temp_config_dir()).unwrap();
    assert_eq!(fs::metadata(imported.path).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn rejects_non_google_hosts_and_mixed_web_credentials() {
    assert_eq!(parse(evil_host_json()).unwrap_err().code, "OAUTH_AUTH_URI_INVALID");
    assert_eq!(parse(mixed_json()).unwrap_err().code, "OAUTH_DESKTOP_CREDENTIAL_REQUIRED");
}
```

Also assert Debug and serialized summaries contain no client secret, full client ID, source filename, URI path/query/fragment, or authorization code.

- [ ] **Step 2: Implement read-validate-write import**

Read source bytes without following a directory, parse only root `{installed:{...}}`, require official HTTPS auth/token hosts and a loopback redirect, then atomically write `<app-config>/youtube/oauth-client.json` using `OpenOptionsExt::mode(0o600)`. Never persist the source path. Re-import invalidates the cached OAuth service but does not silently revoke an existing channel.

- [ ] **Step 3: Port the vault with a new service name**

Keep `SecretString` redacted in Debug. Implement `MemoryTokenVault` for tests. Map `NoEntry` to `AUTH_REQUIRED`; map all other keyring details to `CREDENTIAL_VAULT_ERROR` without formatting the original error.

- [ ] **Step 4: Implement authorized-channel state**

Persist channel ID, safe channel title, authorized timestamp, active ID, and credential configured flag with atomic writes. Revoke removes the selected account only after remote revoke and Keychain deletion succeed; remove-credential is a separate explicit command.

- [ ] **Step 5: Run tests**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml youtube::config youtube::vault youtube::state`

Expected: PASS; a recursive scan of test temp outputs finds no synthetic secret outside the intended 0600 credential fixture.

---

### Task 3: System-Browser PKCE OAuth, Channel Selection, and Revoke

**Files:**
- Create: `desktop/src-tauri/src/youtube/oauth.rs`
- Modify: `desktop/src-tauri/src/youtube/mod.rs`
- Create: `desktop/src-tauri/src/youtube/commands.rs`
- Modify: `desktop/src-tauri/src/lib.rs`

**Interfaces:**
- Produces: `OAuthService::authorize()`, `access_token(channel_id)`, `force_refresh_access_token(channel_id)`, `revoke(channel_id)`.
- Tauri commands: `get_youtube_snapshot`, `import_youtube_oauth_config`, `authorize_youtube`, `set_youtube_channel`, `revoke_youtube`, `remove_youtube_oauth_config`.

- [ ] **Step 1: Port wiremock OAuth tests**

Cover random state, S256 challenge, state mismatch, callback timeout, token exchange, missing refresh token, channel lookup, forced refresh after 401, revoke, and refresh-vs-revoke race. Browser opener is an injected closure that captures the URL without opening a real browser.

- [ ] **Step 2: Run and verify RED**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml youtube::oauth`

Expected: compile failure for missing service.

- [ ] **Step 3: Adapt the proven OAuth flow**

Bind `127.0.0.1:0`, generate high-entropy state/verifier, use PKCE S256, request only `youtube.upload`, open via `tauri-plugin-opener`, accept one bounded HTTP callback, compare state in constant time, exchange code, query `channels?part=snippet&mine=true`, save refresh token, and cache access token until 30 seconds before expiry.

- [ ] **Step 4: Register commands and opener permission**

Commands return only structured summaries. Removing credentials is blocked while an upload is active, and clears the private JSON only after the user explicitly invokes the separate remove command.

- [ ] **Step 5: Run tests and checkpoint**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml youtube::oauth youtube::commands`

Expected: PASS with no browser or Google network access.

---

### Task 4: Per-Job Resumable Upload and Server-Truth Result

**Files:**
- Create: `desktop/src-tauri/src/youtube/upload.rs`
- Modify: `desktop/src-tauri/src/youtube/models.rs`
- Modify: `desktop/src-tauri/src/youtube/commands.rs`
- Modify: `desktop/src-tauri/src/media/model.rs`
- Modify: `desktop/src-tauri/src/media/storage.rs`
- Modify: `desktop/src-tauri/src/media/mod.rs`

**Interfaces:**
- Produces: `ResumableUploader::upload(intent, checkpoint_path, callbacks)`.
- Checkpoint path: `<app-data>/youtube/uploads/<media-job-id>.json`.
- Produces Tauri command: `start_youtube_upload_job(request)`; shared media commands continue to own cancel/retry, and `retry_youtube_thumbnail(job_id)` is added in Task 5.
- Produces media job stages `preparingAuthorization`, `creatingSession`, `uploading`, `waitingToRetry`, `processing`, `settingThumbnail`, terminal states.

- [ ] **Step 1: Port/adapt upload request and checkpoint tests**

Use wiremock/local server tests for session creation, 8 MiB chunks, 308 `Range`, empty status PUT, transport/5xx recovery, 401 forced refresh, cancellation, source mutation, checkpoint corruption, and session URL origin validation.

```rust
#[test]
fn checkpoint_debug_redacts_session_url() {
    let value = format!("{:?}", checkpoint_with("https://upload.example/secret-session"));
    assert!(!value.contains("secret-session"));
}
```

- [ ] **Step 2: Change the old single-checkpoint design to per-job checkpoints**

Checkpoint includes canonical file path, size, modified nanoseconds, uploaded offset, redacted-at-debug session URL, requested metadata hash, and channel ID hash. On resume, reject any mismatch with `UPLOAD_SOURCE_CHANGED` or `UPLOAD_INTENT_CHANGED`.

- [ ] **Step 3: Implement resumable protocol and forced refresh**

Create the session with `part=snippet,status`; upload contiguous chunks; persist the server-confirmed next offset only. On retryable errors, query status before resuming. On 401, force refresh once. On 404 from a status query, return `UPLOAD_SESSION_EXPIRED` and require explicit user restart from zero.

- [ ] **Step 4: Parse server truth**

The terminal response parser requires video ID and reads `status.privacyStatus`. `UploadResult.privacyStatus` comes from the response, not from the requested value. If the server omits status, return the video ID/link plus `privacyStatus: null` and display unknown; never report requested public as actual public.

- [ ] **Step 5: Integrate with the native media worker**

Register `start_youtube_upload_job(request)` in `youtube/commands.rs` and `lib.rs`. The command validates the immutable merged-source snapshot and explicit confirmations before creating the job. YouTube upload is a media job kind but does not consume the heavy local AI/FFmpeg slot after source preflight. Maintain one active upload per account/app, support cancel/retry through the shared media commands, persist progress/checkpoint, and emit `media-job-progress` using the shared job ID.

- [ ] **Step 6: Run tests**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml youtube::upload`

Expected: PASS including 401/308/404 and restart-resume fixtures.

---

### Task 5: Cover Preparation and Retryable Custom Thumbnail

**Files:**
- Create: `desktop/src-tauri/src/youtube/thumbnail.rs`
- Modify: `desktop/src-tauri/src/youtube/upload.rs`
- Modify: `desktop/src-tauri/src/youtube/models.rs`
- Modify: `desktop/src-tauri/src/youtube/commands.rs`
- Modify: `desktop/src-tauri/src/media/model.rs`

**Interfaces:**
- Produces: `prepare_thumbnail(source, tools, temp_dir) -> PathBuf` limited to JPEG/PNG and <= 2 MiB.
- Produces: `set_thumbnail(video_id, path, token)` and `retry_youtube_thumbnail(job_id)`.
- Terminal states distinguish `Completed` from `VideoUploadedThumbnailFailed`.

- [ ] **Step 1: Write preparation and partial-success tests**

```rust
#[tokio::test]
async fn thumbnail_failure_preserves_successful_video_and_retries_only_thumbnail() {
    let result = upload_then_thumbnail(mock_video_201(), mock_thumbnail_403()).await.unwrap();
    assert_eq!(result.status, UploadCompletion::VideoUploadedThumbnailFailed);
    assert_eq!(result.video_id.as_deref(), Some("video-1"));
    retry_thumbnail(result.job_id).await.unwrap();
    assert_eq!(mock_video_upload_calls(), 1);
    assert_eq!(mock_thumbnail_calls(), 2);
}
```

Test JPEG/PNG passthrough under 2 MiB, conversion/compression, missing cover fallback, invalid image, permission 403, and rate-limit 429.

- [ ] **Step 2: Implement cover acquisition/preparation**

Use the cached/local batch cover when available. If the snapshot contains only an HTTPS cover URL, download it through the native client into the job temp directory with a bounded size/content-type policy. Preserve the image content; use bundled FFmpeg only to convert/compress to JPEG/PNG under 2 MiB. Missing/invalid cover yields `thumbnailState: skipped` but does not block video upload after explicit UI warning.

- [ ] **Step 3: Implement `thumbnails.set`**

After video success, POST media to the official endpoint with `videoId`. Validate the fixed official host, handle 401 with one forced refresh, and map 403/429 to stable thumbnail codes. Never delete video/upload checkpoint data needed to display partial success.

- [ ] **Step 4: Implement thumbnail-only retry**

Store video ID, cover snapshot, and safe thumbnail error on the completed upload job. Retry checks that the video ID and cover still match, then calls only `set_thumbnail`.

- [ ] **Step 5: Run tests**

Run: `cargo test --manifest-path desktop/src-tauri/Cargo.toml youtube::thumbnail youtube::upload`

Expected: PASS.

---

### Task 6: YouTube Settings and Typed Frontend Hook

**Files:**
- Create: `desktop/src/youtube/types.ts`
- Create: `desktop/src/youtube/commands.ts`
- Create: `desktop/src/youtube/useYouTube.ts`
- Create: `desktop/src/youtube/useYouTube.test.tsx`
- Create: `desktop/src/settings/YouTubeSettings.tsx`
- Create: `desktop/src/settings/YouTubeSettings.test.tsx`
- Modify: `desktop/src/settings/SettingsPage.tsx`
- Modify: `desktop/src/api.ts`
- Modify: `desktop/src/App.tsx`
- Modify: `desktop/src/preview.ts`

**Interfaces:**
- Produces: `YouTubeModel { snapshot, loading, authorizing, importCredential, authorize, setActiveChannel, revoke, removeCredential }`.
- Centralizes normalization of `{code,message}`; unknown native errors become `UNKNOWN_ERROR` without raw values.

- [ ] **Step 1: Write failing hook/settings tests**

```tsx
it("shows prerequisites without rendering secrets", async () => {
  render(<YouTubeSettings model={configuredAuthorizedModel} />);
  expect(screen.getByText("已授权")).toBeInTheDocument();
  expect(screen.queryByText(/client_secret|refresh_token|synthetic-secret/i)).toBeNull();
});
```

Test unconfigured, configured/not authorized, system-browser authorization pending, channel switch, revoke, remove-credential confirmation, and stable errors.

- [ ] **Step 2: Implement typed command module**

Keep every `invoke()` in `youtube/commands.ts`; Settings components receive callbacks only. The import action uses the dialog plugin filtered to JSON, passes the selected path once to Rust, then immediately drops it from React state.

- [ ] **Step 3: Build the YouTube settings card**

Display only configured status and safe channel title/authorization time. Show the fixed sequence `导入凭证 -> 授权 YouTube -> 选择频道`. Do not enable authorization without a valid private copy.

- [ ] **Step 4: Run tests/build**

Run: `cd desktop && npm test -- src/youtube src/settings/YouTubeSettings.test.tsx && npm run build`

Expected: PASS and no secret-shaped fixture strings in rendered snapshots.

---

### Task 7: Upload Dialog, Metadata Inheritance, and Upload Job UI

**Files:**
- Create: `desktop/src/components/YouTubeUploadDialog.tsx`
- Create: `desktop/src/components/YouTubeUploadDialog.test.tsx`
- Create: `desktop/src/components/YouTubeUploadJobs.tsx`
- Create: `desktop/src/components/YouTubeUploadJobs.test.tsx`
- Modify: `desktop/src/components/DownloadManagerPage.tsx`
- Modify: `desktop/src/components/DownloadManagerPage.test.tsx`
- Modify: `desktop/src/media/types.ts`
- Modify: `desktop/src/media/commands.ts`
- Modify: `desktop/src/media/useMediaJobs.ts`
- Modify: `desktop/src/App.tsx`
- Modify: `desktop/src/preview.ts`
- Modify: `desktop/src/styles.css`

**Interfaces:**
- Produces `buildUploadDefaults(batch, mediaOutputs) -> UploadDraft`.
- Upload button enabled only for authorized YouTube plus validated merged video.
- `startYouTubeUpload(intent)` creates a native media upload job; subtitles are never included.

- [ ] **Step 1: Write exact default-mapping tests**

```ts
it("inherits download metadata and prefers the no-music merged video", () => {
  expect(buildUploadDefaults(batch, outputs)).toMatchObject({
    filePath: outputs.noBackgroundMusicMerged,
    title: batch.series.title,
    description: batch.series.abstract,
    cover: batch.series.cover,
    privacyStatus: "public",
    selfDeclaredMadeForKids: false,
    containsSyntheticMedia: true,
  });
});

it("uses the title as the only description fallback", () => {
  expect(buildUploadDefaults(batchWithoutAbstract, outputs).description).toBe(batch.series.title);
});
```

Also test non-AI drama synthetic default false, original-audio fallback with visible warning, editable values, title required, audience/synthetic/final confirmation, and no caption field/API call.

- [ ] **Step 2: Run and verify RED**

Run: `cd desktop && npm test -- src/components/YouTubeUploadDialog.test.tsx`

Expected: missing component/helper failure.

- [ ] **Step 3: Implement upload dialog**

Render source selector, title, description, cover preview/replace, tags, category selector initially set to Entertainment (`24`), privacy initially public, audience declaration, synthetic-content declaration, copyright/community confirmation, and final publish confirmation. Show the unverified-project private-mode warning beside privacy selection.

- [ ] **Step 4: Implement upload job rows**

Populate the existing `YouTube 上传` tab with channel, title, source, progress, waiting/retry/processing stages, cancel, resume/restart, video ID/link, actual privacy, thumbnail state, and thumbnail-only retry. Never show upload session URL or bearer/token errors.

- [ ] **Step 5: Connect explicit start and result handling**

The button only opens the dialog; closing/cancelling creates no job. Confirming snapshots source/cover and calls `start_youtube_upload_job`. Completion reads native server-truth result; it does not infer public from the draft.

- [ ] **Step 6: Run UI regression and browser checkpoint**

Run: `cd desktop && npm test -- src/components/YouTubeUploadDialog.test.tsx src/components/YouTubeUploadJobs.test.tsx src/components/DownloadManagerPage.test.tsx src/youtube && npm run build`

Expected: PASS. Preview must show ready, uploading, partial thumbnail failure, forced-private result, and unauthorized states without native calls.

---

### Task 8: Plan-3 Security, Regression, and User-Operated Live Boundary

**Files:**
- Modify only listed files when a failing acceptance check requires a fix.

**Interfaces:**
- Validates all YouTube interfaces and preserves Plans 1–2.

- [ ] **Step 1: Run all automatic tests**

```bash
python3 -m unittest discover -s tests -v
python3 -m unittest discover -s ai_worker/tests -v
cd desktop && npm test && npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: every command exits 0.

- [ ] **Step 2: Run a secret-leak scan with synthetic fixtures**

Scan source-generated logs, `desktop/dist`, Tauri bundle, media job JSON, settings JSON, notification fixtures, and test snapshots for the complete synthetic client ID, client secret, refresh/access token, authorization code, and session URL. Field names such as `client_secret` are not themselves proof of a leak; full synthetic values must be absent outside the intentionally protected 0600 test fixture.

- [ ] **Step 3: Build and launch packaged app**

Run: `cd desktop && npm run tauri build`. Launch `.app`, import a synthetic invalid credential to verify safe rejection, and confirm no credential JSON is inside `.app`/DMG.

- [ ] **Step 4: Stop at the real Google consent boundary**

For real validation, hand control to the user to import their private Desktop OAuth JSON, log in, select a channel, and consent. Do not automate or click consent on the user's behalf.

- [ ] **Step 5: User performs a disposable real upload**

The user chooses a disposable merged test video and confirms publication. Observe progress, actual privacy, link, cover result, cancellation/resume if safely reproducible, and YouTube processing. Report real OAuth/upload as verified only when direct runtime evidence exists.

- [ ] **Step 6: Record checkpoint**

Record automated counts, bundle/artifact checksums, redaction result, mocked protocol coverage, and whether live authorization/upload was completed by the user. Do not claim captions were uploaded.
