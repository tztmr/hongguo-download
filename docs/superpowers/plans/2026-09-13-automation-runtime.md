# Automation Runtime Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task-by-task in this session.

**Goal:** Make the saved five-step automation runnable with persistent progress and explicit controls.
**Architecture:** A Tauri-owned service stores settings and jobs atomically, polls feeds, and advances durable per-series stages using existing native media and YouTube services. React subscribes/polls snapshots without owning execution.
**Tech Stack:** Rust / Tokio / serde / existing Tauri and Python sidecar; React / TypeScript.
**Spec:** docs/superpowers/specs/2026-09-13-automation-runtime-design.md

## Global Constraints
- Preserve existing source and unrelated dirty API binary. No Windows CI.
- Two independent AI keys in OS vault; no secrets in persisted draft or logs.
- No automatic public bulk run during verification.
- Confirmed duplicates skip; uncertain results require review; retain season identity.
- Pause/stop never delete files. Cleanup follows all required upload outcomes.

### Task 1: Persist settings and state with controls
Files: desktop/src-tauri/src/automation/{mod,model,storage}.rs; desktop/src-tauri/src/lib.rs.
- [x] Validate all existing controls; migrate old frontend draft through save command.
- [x] Atomic state transaction; protect duplicate starts with service mutex and running job set.
- [x] Expose snapshot/save/start/control/review commands; load saved mode at app startup.
- [x] Test invalid configuration, restart recovery, pause/stop, duplicate task identities, redacted secrets.

### Task 2: Advance real tasks
Files: desktop/src-tauri/src/automation/{runner,source,metadata}.rs.
- [x] Paginate feeds, filter source metadata, fetch catalogue, check duplicates.
- [x] Persist per-episode downloads in named private series roots; disk threshold and valid video checks.
- [x] Recover native merge/separation/subtitle jobs from recorded IDs and deterministic request identity.
- [x] Generate fixed-prompt AI metadata and thumbnail; store results; fallback without key.
- [x] Deterministic main/Shorts upload IDs and record processing outcomes; pause native jobs with runner.
- [x] Cleanup owned media only after required outputs succeed; preserve history and subtitle option.
- [x] Test AI request-boundary fallback/cache, source and format decisions, native retry/restart handling, and real-file cleanup.

### Task 3: Operable UI
Files: desktop/src/monitor/{automationRuntime,AutomationPage,AIStudio}.tsx/ts and automation.css.
- [x] Load saved backend config with old local draft fallback; persist keys separately via existing inputs.
- [x] Save/start/pause/resume/stop/scan controls and clear mode/error text.
- [x] Show per-task stage, progress, log, YouTube links and review/retry actions.
- [x] Test start uses saved inputs, unsaved guards, controls, possible duplicate review and errors.

### Task 4: Validate and deliver
- [x] Run backend/frontend regression and build checks; inspect rendered native controls.
- [x] Test native main/Shorts media jobs with a local fixture, native save/start/scan/pause/resume/stop in isolated QA, and HTTP processing mocks; document remote publishing proof separately.
- [x] Build macOS locally for user installation; no CI/Windows. Report exactly what ran.
