# Unlimited Today Monitor and Ranking Feeds Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the capped candidate monitor and mismatched ranking requests with the captured date feed, three typed monitor sources, dynamic secondary categories, and pagination that has no application-level total limit.

**Architecture:** A new pure `core/duanju_feeds.py` module owns captured query construction and response normalization. FastAPI owns signed calls, sticky-device cursor state, batching and loop-safety; React owns per-day typed persistence, full refresh merging and local category filtering.

**Tech Stack:** Python 3.11, FastAPI, httpx, unittest, React 19, TypeScript 5.8, Vitest, Tauri 2.

**Spec:** `docs/superpowers/specs/2026-09-02-captured-short-drama-feeds-rewrite-design.md`

## Global Constraints

- Monitor types are exactly `playlet`, `comic_series_rank`, and `ai_playlet`.
- All date comparisons use `Asia/Shanghai`; clients cannot choose the monitored date.
- No application total-count or total-page cap; `limit=20` is only the response page size.
- Stop malformed infinite pagination by rejecting repeated state, non-advancing offsets, and empty pages that claim more data.
- Never persist or log Reqable credentials, fixed capture devices, session values, signatures, or full upstream URLs.
- This directory is not a Git repository, so checkpoint commits/worktrees are unavailable; preserve unrelated files and report that boundary.

---

### Task 1: Captured Feed Protocol Module

**Files:**
- Create: `core/duanju_feeds.py`
- Create: `tests/test_duanju_feeds.py`

**Interfaces:**
- Produces: `TODAY_RELEASE_TYPES`, `RANK_BOARDS`, `build_subscribe_url(...)`, `build_rank_url(...)`, `parse_subscribe_page(...)`, `parse_rank_page(...)`, and `parse_batch_metrics(...)`.
- Normalized items include `series_id`, `book_id`, `title`, media fields, metrics, `online_time`, `release_type`, and `category_tags`.

- [ ] **Step 1: Write failing protocol tests**

Add hand-built fixtures proving the subscribe parser uses `schedule_publish_time`, rejects offline/wrong-date rows, preserves zero metrics, and labels rows as `playlet`. Add rank fixtures proving direct/nested video layouts parse, selector source becomes the release type, and rank pagination includes `rank_version`.

- [ ] **Step 2: Run tests and verify RED**

Run: `python3 -m unittest tests.test_duanju_feeds -v`

Expected: import failure because `core.duanju_feeds` does not exist.

- [ ] **Step 3: Implement the pure protocol functions**

Use `urllib.parse.urlencode` and only the captured public business parameters. Parse recursively only where the upstream has two observed video layouts. Do not return `recommend_info` or any raw response.

- [ ] **Step 4: Run tests and verify GREEN**

Run: `python3 -m unittest tests.test_duanju_feeds -v`

Expected: all protocol tests pass.

### Task 2: Unlimited Safe Collector and Three-Type Endpoint

**Files:**
- Modify: `core/new_releases.py`
- Modify: `endpoints/duanju.py`
- Modify: `tests/test_new_releases.py`
- Modify: `tests/test_duanju_extended.py`

**Interfaces:**
- `collect_today_releases(...)` keeps reading until it fills the requested transport page or upstream ends, with repeated-state detection instead of `MAX_UPSTREAM_PAGES`.
- `/api/duanju/new-releases` accepts only the three monitor types.
- `playlet` calls the date log directly; comic/AI call their typed new-rank source and batch-enrich each upstream page.

- [ ] **Step 1: Write the failing unlimited collector test**

Create 12 empty-but-progressing old-date pages followed by one today row and assert 13 calls, one returned row, and no hard stop at page 10. Add a repeated-state fixture and assert `RuntimeError("上游分页状态未前进")`.

- [ ] **Step 2: Run collector tests and verify RED**

Run: `python3 -m unittest tests.test_new_releases.CursorAndCollectorTests -v`

Expected: the old ten-page test fails because the desired collector must continue past ten.

- [ ] **Step 3: Implement progress-based loop safety**

Remove `MAX_UPSTREAM_PAGES`; derive a stable marker from upstream state and reject a repeated marker. Keep buffered matches and short cursor behavior unchanged.

- [ ] **Step 4: Write failing route tests**

Assert `playlet` sends `target_date=20260902`, the captured subscribe parameters and `aid=8662`. Assert comic and AI send distinct selectors, call metadata in comma-separated batches rather than once per series, retain `release_type`, and reject `all`/`adaptation`.

- [ ] **Step 5: Run route tests and verify RED**

Run: `python3 -m unittest tests.test_duanju_extended.DuanjuExtendedTests -v`

Expected: current candidate-cell URLs and per-series metadata calls violate the new assertions.

- [ ] **Step 6: Wire the new protocol module into FastAPI**

Keep the existing endpoint path. Store sticky dynamic device, offset, session, rank version and recent IDs in the existing opaque cursor store. Use one batch metadata POST per typed rank page.

- [ ] **Step 7: Run backend tests and verify GREEN**

Run: `python3 -m unittest discover -s tests -v`

Expected: all Python tests pass.

### Task 3: Persistent Three-Type Monitor and Secondary Categories

**Files:**
- Modify: `desktop/src/types.ts`
- Modify: `desktop/src/api.ts`
- Modify: `desktop/src/monitor/storage.ts`
- Modify: `desktop/src/monitor/storage.test.ts`
- Create: `desktop/src/monitor/categories.ts`
- Create: `desktop/src/monitor/categories.test.ts`
- Modify: `desktop/src/monitor/useNewReleaseMonitor.ts`
- Modify: `desktop/src/monitor/useNewReleaseMonitor.test.tsx`
- Modify: `desktop/src/monitor/NewReleasesPage.tsx`
- Modify: `desktop/src/monitor/NewReleasesPage.test.tsx`

**Interfaces:**
- `NewReleaseType` is the three-value union.
- `releaseCategoryNames(item)` returns normalized category labels.
- Storage reads/writes full per-day typed result snapshots, pruning stale days.
- A refresh consumes every backend cursor page, merges unique rows with persisted rows, sorts by newest time, and notifies only after the silent baseline.

- [ ] **Step 1: Write failing category and storage tests**

Assert `青春校园` becomes `校园`, `古装/仙侠` becomes `古风`, unknown useful tags remain visible, and empty tags become `其他`. Assert typed item snapshots survive a new hook instance and stale-date snapshots are ignored.

- [ ] **Step 2: Run tests and verify RED**

Run: `npm test -- --run src/monitor/categories.test.ts src/monitor/storage.test.ts` from `desktop/`.

Expected: missing category helper and snapshot APIs.

- [ ] **Step 3: Implement category normalization and dated snapshot storage**

Use a versioned localStorage key and JSON validation. Store only the current Shanghai date for each type.

- [ ] **Step 4: Write failing hook and component tests**

Assert refresh consumes three pages without a hop cap, retains previously persisted IDs that disappear upstream, exposes only three type buttons, filters by `校园`/`古风`/`其他`, and preserves old rows on refresh failure.

- [ ] **Step 5: Run tests and verify RED**

Run: `npm test -- --run src/monitor/useNewReleaseMonitor.test.tsx src/monitor/NewReleasesPage.test.tsx` from `desktop/`.

Expected: current hook replaces rows with the first 20 and current UI includes `全部`/`改编剧` without secondary filters.

- [ ] **Step 6: Implement the monitor hook and UI**

Default to `playlet`; fully drain cursor pages during scheduled/manual refresh; merge by `bookId`; expose `categories`, `selectedCategory`, `filteredItems`, `setCategory`; and display the loaded count plus refresh time.

- [ ] **Step 7: Run monitor tests and verify GREEN**

Run: `npm test -- --run src/monitor` from `desktop/`.

Expected: all monitor tests pass.

### Task 4: Capture-Exact Eight-Board Ranking Without NO.100 Cap

**Files:**
- Modify: `endpoints/duanju.py`
- Modify: `tests/test_duanju_extended.py`
- Modify: `desktop/src/types.ts`
- Modify: `desktop/src/api.ts`
- Modify: `desktop/src/App.tsx`
- Modify: `desktop/src/App.feed-search.test.tsx`
- Modify: `desktop/src/feed/groupedPaging.test.ts`

**Interfaces:**
- `/api/duanju/rank?board=&cursor=&limit=20` returns opaque pagination and the fixed 8-board catalog.
- React no longer chooses rank group/category or passes upstream state.

- [ ] **Step 1: Write failing rank protocol and API tests**

Assert the first call uses `selected_items=all`, `client_template=2`, `client_req_type=2`, and selector-change type 2. Assert a later cursor call uses selector-change type 1 and carries rank version, session, offset, recent IDs and sticky device. Assert all eight board IDs are returned.

- [ ] **Step 2: Run rank tests and verify RED**

Run: `python3 -m unittest tests.test_duanju_extended.DuanjuExtendedTests -v`

Expected: current iOS Redfruit profile, first-page request mode and exposed pagination contract fail.

- [ ] **Step 3: Implement opaque rank aggregation**

Aggregate upstream pages into 20-row transport pages with buffer, cursor binding and progress checks. Reject unknown boards and cross-board cursors.

- [ ] **Step 4: Write failing React rank regression**

Replace the old “stops at NO.100” assertion with 12 ten-row upstream responses and assert scrolling reaches `NO.120`, while only the eight board buttons are rendered.

- [ ] **Step 5: Run React test and verify RED**

Run: `npm test -- --run src/App.feed-search.test.tsx` from `desktop/`.

Expected: current `maxItems: 100` prevents `NO.101` and later rows.

- [ ] **Step 6: Remove the cap and simplify rank controls**

Delete `{ maxItems: 100 }` from rank paging, remove rank group/category state and UI, and use only `board/cursor/limit` through the API adapter.

- [ ] **Step 7: Run rank/frontend tests and verify GREEN**

Run: `npm test -- --run src/App.feed-search.test.tsx src/feed/groupedPaging.test.ts` from `desktop/`.

Expected: rank reaches 120 in the fixture and generic paging remains uncapped by default.

### Task 5: Full Regression, Build and Rendered QA

**Files:**
- Modify only if a regression exposes a task-scoped defect.
- Store screenshots outside the repository.

**Interfaces:**
- Produces fresh test/build/runtime evidence for the handoff.

- [ ] **Step 1: Run all Python tests**

Run: `python3 -m unittest discover -s tests -v`

- [ ] **Step 2: Run all React tests and production build**

Run: `npm test && npm run build` from `desktop/`.

- [ ] **Step 3: Run Rust checks and Tauri build**

Run the repository's existing Cargo/Tauri commands discovered from `desktop/src-tauri/` without changing dependency versions.

- [ ] **Step 4: Run rendered interaction QA**

The flow under test is: open the app -> enter 新剧监听 -> switch among 真人剧/漫剧/AI剧 -> select a secondary category -> see only matching today rows; then enter 榜单 -> scroll beyond the initial group -> numbering continues without an application cap.

Use the available in-app Browser skill first. Check page identity, meaningful DOM, no framework overlay, console errors/warnings, screenshots, and both target interactions.

- [ ] **Step 5: Re-read the approved spec and report boundaries**

Confirm every user-visible requirement has a test or runtime observation. Explicitly distinguish live upstream natural window size from application limits and report that closed-app system monitoring remains outside scope.
