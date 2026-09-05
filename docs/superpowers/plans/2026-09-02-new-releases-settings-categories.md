# New Releases, Settings, Categories, and Notifications Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add real series metrics, Beijing-today new-release monitoring, persistent download settings, macOS notifications, grouped homepage categories, and exact 5-column/20-item paging across the desktop app.

**Architecture:** FastAPI owns all signed upstream requests and exposes normalized metrics/new-release/category models; the Tauri layer persists native settings and enables macOS notifications; focused React components and hooks render settings, monitoring, categories, and exact group pagination. Existing pure signing, device pool, auto-registration, failover, download queue, and stale-request protection remain the only runtime call paths.

**Tech Stack:** Python 3.11, FastAPI, httpx, unittest, React 19, TypeScript 5.8, Vitest, Testing Library, Tauri 2.8, Rust 2021, `tauri-plugin-notification` 2.

**Spec:** `docs/superpowers/specs/2026-09-02-new-releases-settings-categories-design.md`

## Global Constraints

- Never copy `/Users/edking/Desktop/all番茄.reqable_collection.json` into the project or tests.
- Never log or render raw device IDs, signatures, cookies, tokens, or complete upstream query strings.
- Every upstream call must reuse `PureSignedClient`, the device pool, automatic registration, and failover.
- `0` is a valid metric; only missing values become `null` / `undefined` and render as `—`.
- “Today” always means `Asia/Shanghai` from `00:00:00` through `23:59:59`.
- Home, search, rank, and monitor use 5 desktop columns and reveal at most 20 new cards per group.
- New-release metrics are enriched with concurrency 4 and cached for 5 minutes.
- The app monitors every 5 minutes only while open; first load establishes a silent baseline.
- Current workspace is not a Git repository. Replace every normal commit checkpoint with a status/test checkpoint and do not initialize Git without explicit user authorization.

---

## File Map

### Python backend

- Create `core/new_releases.py`: pure metric parsing, Beijing-day filtering, bounded short-cursor storage, and async 20-item collection.
- Create `tests/test_new_releases.py`: deterministic tests for timestamps, metrics, cursors, filtering, concurrency behavior, and paging.
- Modify `endpoints/duanju.py`: signed candidate/category/metadata requests, new public endpoints, cache integration, and `definition=auto`.
- Create `tests/test_duanju_extended.py`: endpoint-facing parser/profile tests with sanitized fixtures and fake clients.
- Modify `API_USAGE.md`: document normalized metrics, categories, new releases, and auto definition.

### Tauri/native

- Create `desktop/src-tauri/src/settings.rs`: versioned settings model plus atomic load/save.
- Modify `desktop/src-tauri/src/lib.rs`: settings-backed app state and Tauri commands.
- Modify `desktop/src-tauri/Cargo.toml`: notification plugin dependency.
- Modify `desktop/src-tauri/capabilities/default.json`: notification permission.
- Modify `desktop/package.json` and `desktop/package-lock.json`: JavaScript notification plugin.

### React frontend

- Modify `desktop/src/types.ts`: metrics, category groups, settings, monitor page, and new nav IDs.
- Modify `desktop/src/api.ts`: metrics, categories, new releases, settings, and notification-safe adapters.
- Create `desktop/src/feed/groupedPaging.ts` and `.test.ts`: exact 20-result accumulator with de-duplication and hidden-buffer support.
- Create `desktop/src/components/CategoryFilter.tsx` and `.test.tsx`: grouped horizontal filters and expand/collapse.
- Modify `desktop/src/components/SeriesInspector.tsx` and `.test.tsx`: real series metrics and selected resolution copy.
- Create `desktop/src/settings/useAppSettings.ts`, `SettingsPage.tsx`, and tests.
- Create `desktop/src/monitor/storage.ts`, `useNewReleaseMonitor.ts`, `NewReleasesPage.tsx`, and tests.
- Create `desktop/src/notifications.ts` and `.test.ts`: permission and aggregate notification adapter.
- Modify `desktop/src/download/model.ts`, `storage.ts`, `useDownloadManager.ts`, and tests: selected definition plus exactly-once batch completion.
- Modify `desktop/src/components/AppRail.tsx` and `.test.tsx`: monitor/settings entries and new-release badge.
- Modify `desktop/src/App.tsx`, `preview.ts`, and app tests: composition and stale-request-safe data flow.
- Modify `desktop/src/styles.css`: 5-column cards, grouped filters, monitor cards, settings page, and responsive guardrails.

---

### Task 1: Pure New-Release Domain Functions

**Files:**
- Create: `core/new_releases.py`
- Create: `tests/test_new_releases.py`

**Interfaces:**
- Produces: `normalize_metrics(upstream, series_id) -> dict`, `is_shanghai_today(timestamp, now) -> bool`, a bounded `CursorStore`, and `collect_today_releases(...) -> dict`.
- Consumes: only standard library (`asyncio`, `base64`, `datetime`, `json`, `secrets`, `time`, `zoneinfo`).

- [ ] **Step 1: Write failing tests for metric zero/missing semantics and Beijing-day boundaries**

```python
from datetime import datetime
from zoneinfo import ZoneInfo
import unittest

from core.new_releases import is_shanghai_today, normalize_metrics

class NewReleaseDomainTests(unittest.TestCase):
    def test_normalize_metrics_preserves_zero_and_missing(self):
        upstream = {"data": {"series": {
            "series_id": "s1", "create_time": 1788307200,
            "series_play_cnt": 0, "hot_score": 20,
            "followed_cnt": None, "digg_cnt": 0,
        }}}
        value = normalize_metrics(upstream, "s1")
        self.assertEqual(value["play_count"], 0)
        self.assertEqual(value["hot_count"], 20)
        self.assertIsNone(value["collect_count"])
        self.assertEqual(value["like_count"], 0)

    def test_today_uses_shanghai_calendar_boundaries(self):
        now = datetime(2026, 9, 2, 15, 0, tzinfo=ZoneInfo("Asia/Shanghai"))
        self.assertTrue(is_shanghai_today(int(datetime(2026, 9, 2, 0, 0, tzinfo=now.tzinfo).timestamp()), now))
        self.assertTrue(is_shanghai_today(int(datetime(2026, 9, 2, 23, 59, 59, tzinfo=now.tzinfo).timestamp()), now))
        self.assertFalse(is_shanghai_today(int(datetime(2026, 9, 1, 23, 59, 59, tzinfo=now.tzinfo).timestamp()), now))
```

- [ ] **Step 2: Run the tests and verify RED**

Run: `python3 -m unittest tests.test_new_releases.NewReleaseDomainTests -v`

Expected: import failure because `core.new_releases` does not exist.

- [ ] **Step 3: Implement recursive metadata discovery and strict numeric conversion**

```python
SHANGHAI = ZoneInfo("Asia/Shanghai")

def _walk(value):
    if isinstance(value, dict):
        yield value
        for child in value.values():
            yield from _walk(child)
    elif isinstance(value, list):
        for child in value:
            yield from _walk(child)

def _optional_int(value):
    if value is None or value == "":
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None

def normalize_metrics(upstream: dict, series_id: str) -> dict:
    match = next((item for item in _walk(upstream)
                  if str(item.get("series_id") or item.get("series_id_str") or "") == series_id
                  and "create_time" in item), {})
    return {
        "online_time": _optional_int(match.get("create_time")),
        "play_count": _optional_int(match.get("series_play_cnt")),
        "hot_count": _optional_int(match.get("hot_score")),
        "collect_count": _optional_int(match.get("followed_cnt")),
        "like_count": _optional_int(match.get("digg_cnt")),
    }
```

Implement `is_shanghai_today` by comparing `datetime.fromtimestamp(timestamp, SHANGHAI).date()` with the supplied/current Shanghai date; invalid/non-positive timestamps return `False`.

- [ ] **Step 4: Add failing cursor-store and collector tests**

```python
class CursorAndCollectorTests(unittest.IsolatedAsyncioTestCase):
    def test_cursor_is_short_and_rejects_wrong_type_date_or_expiry(self):
        now = [1000]
        store = CursorStore(ttl_seconds=600, clock=lambda: now[0])
        token = store.put("all", "2026-09-02", {"upstream": 18, "seen": ["s1"], "buffer": []})
        self.assertLess(len(token), 200)
        self.assertEqual(store.get(token, "all", "2026-09-02")["upstream"], 18)
        with self.assertRaisesRegex(ValueError, "分页游标已失效"):
            store.get(token, "playlet", "2026-09-02")
        with self.assertRaisesRegex(ValueError, "分页游标已失效"):
            store.get(token, "all", "2026-09-03")
        now[0] = 1601
        with self.assertRaisesRegex(ValueError, "分页游标已失效"):
            store.get(token, "all", "2026-09-02")

    async def test_collects_twenty_unique_today_items_across_eighteen_item_pages(self):
        pages = [
            {"items": [{"series_id": f"s{i}"} for i in range(1, 19)], "next": 18, "has_more": True},
            {"items": [{"series_id": f"s{i}"} for i in range(19, 37)], "next": 36, "has_more": False},
        ]
        today_noon = int(datetime(2026, 9, 2, 12, tzinfo=ZoneInfo("Asia/Shanghai")).timestamp())
        async def fetch_page(state): return pages.pop(0)
        async def fetch_metrics(item):
            return {"online_time": today_noon, "play_count": 1, "hot_count": 2,
                    "collect_count": 3, "like_count": 4}
        result = await collect_today_releases(
            fetch_page=fetch_page, fetch_metrics=fetch_metrics,
            release_type="all", cursor="", limit=20,
            now=datetime(2026, 9, 2, 12, tzinfo=ZoneInfo("Asia/Shanghai")),
        )
        self.assertEqual(len(result["items"]), 20)
        self.assertEqual(len({item["series_id"] for item in result["items"]}), 20)
```

- [ ] **Step 5: Run cursor/collector test and verify RED**

Run: `python3 -m unittest tests.test_new_releases.CursorAndCollectorTests -v`

Expected: failure because cursor and collector functions are undefined.

- [ ] **Step 6: Implement short opaque cursor validation and bounded async enrichment**

Do not put candidate rows, cover URLs, or the growing `seen` set in the query string. `CursorStore` keeps `{upstream, seen, buffer}` in a bounded 10-minute in-memory TTL cache and returns a URL-safe random token whose encoded payload contains only `version`, `type`, `date`, and a 128-bit cache key. A missing/expired entry or mismatched version/type/date raises `ValueError("分页游标已失效")`. Cap stored cursors and evict oldest/expired entries so an abandoned scroll session cannot grow memory without bound. Server restart invalidation is acceptable and becomes the documented HTTP 400 cursor-expired response.

`collect_today_releases` must:

```python
MAX_UPSTREAM_PAGES = 10
METADATA_CONCURRENCY = 4

semaphore = asyncio.Semaphore(METADATA_CONCURRENCY)
async def enrich(item):
    async with semaphore:
        try:
            return {**item, **await fetch_metrics(item)}
        except Exception:
            return {**item, "online_time": None, "play_count": None,
                    "hot_count": None, "collect_count": None, "like_count": None}
```

It filters by Shanghai date, de-duplicates by `series_id`, sorts newest first, returns at most `limit`, and keeps any over-fetched matching rows in the cursor buffer so the next group is still exactly 20 when available.

- [ ] **Step 7: Run the complete new-release domain suite**

Run: `python3 -m unittest tests.test_new_releases -v`

Expected: all tests pass.

- [ ] **Step 8: Record checkpoint**

Record changed paths and test counts in the task log; no Git action because the workspace is not a repository.

---

### Task 2: Signed Metadata and Today-New-Release Endpoints

**Files:**
- Modify: `endpoints/duanju.py`
- Create: `tests/test_duanju_extended.py`
- Modify: `API_USAGE.md`

**Interfaces:**
- Consumes: Task 1 domain functions.
- Produces: `GET /api/duanju/series-metrics` and `GET /api/duanju/new-releases`.
- Reuses: `request.app.state.client.call_with_device(..., method="POST", data=..., aid=8662)`.

- [ ] **Step 1: Write failing sanitized parser/profile tests**

```python
class DuanjuExtendedTests(unittest.TestCase):
    def test_metadata_body_contains_only_stable_business_fields(self):
        body = json.loads(_series_metadata_body("s1", 1004))
        self.assertEqual(body["series_id"], "s1")
        self.assertEqual(body["content_type"], 1004)
        self.assertFalse(body["biz_param"]["disable_digg_stat"])
        self.assertNotIn("device_id", body)

    def test_new_candidate_parser_keeps_content_type(self):
        upstream = {"data": {"cell_view": {"cell_data": [{"video_data": [{
            "series_id": "s1", "title": "新剧", "content_type": 1004,
            "cover": "https://example.invalid/cover", "episode_cnt": 60,
        }]}]}}}
        self.assertEqual(_new_release_candidates(upstream)[0]["content_type"], 1004)
```

- [ ] **Step 2: Run and verify RED**

Run: `python3 -m unittest tests.test_duanju_extended.DuanjuExtendedTests -v`

Expected: missing helper failures.

- [ ] **Step 3: Implement signed metadata retrieval with existing failover**

Add a 300-second TTL cache and body builder:

```python
SERIES_METADATA_URL = f"{BOOKMALL_API}/novel/player/multi_video_detail/preload/v1/"

def _series_metadata_body(series_id: str, content_type: int) -> str:
    return json.dumps({
        "series_id": series_id,
        "content_type": content_type,
        "biz_param": {
            "use_os_player": False, "device_level": 3, "source": 0,
            "disable_video_relate_book": False, "screen_width_px": "828",
            "disable_digg_stat": False, "detail_page_version": 0,
            "need_mp4_align": False, "video_platform": 0,
            "need_all_video_definition": False, "video_id_type": 1,
            "use_server_dns": False,
        },
    }, ensure_ascii=False, separators=(",", ":"))
```

Build the URL with stable bookmall profile parameters plus dynamic `device_id`; call `call_with_device` using POST, JSON body, `aid=8662`, and existing retry/failover. Do not add request headers from the Reqable file.

- [ ] **Step 4: Implement `GET /duanju/series-metrics`**

Parameters: `series_id` (required) and `content_type` (positive integer, compatibility default 1). Return only the normalized five fields plus `series_id` and `content_type`, not raw upstream payload. Extend every normalized catalog/search/rank candidate with its numeric upstream `content_type` (falling back to 1 only when upstream omits it), so normal frontend calls do not rely on that compatibility default.

- [ ] **Step 5: Implement candidate sources and `GET /duanju/new-releases`**

For `type=all`, use the captured stable business selector `selected_items=firstonlinetime_new`, `cell_id=7431550523368554558`, `change_type=1`, and upstream `limit=18`.

For typed filters, reuse the rank candidate protocol with:

```python
TYPE_TO_SELECTED_ITEMS = {
    "playlet": "playlet",
    "comic_series_rank": "comic_series_rank",
    "ai_playlet": "ai_playlet",
    "adaptation": "adaptation",
}
sub_selected_items = "ranklist_new_rank_sc"
```

Pass candidate and metadata fetch closures to `collect_today_releases`; validate `limit` with FastAPI `ge=1, le=20`; convert invalid cursors into HTTP 400 envelope errors rather than 500.

- [ ] **Step 6: Add fake-client endpoint tests**

Use a fake `call_with_device` that asserts `aid == 8662`, records method/body, and returns sanitized candidate/metadata payloads. Test:

- a 20-item response after two 18-item pages;
- a day with only 3 matching items;
- one metadata failure produces null metrics without failing the group;
- `type=ai_playlet` uses `ranklist_new_rank_sc`;
- raw upstream fields are absent from the response.

- [ ] **Step 7: Run backend tests**

Run: `python3 -m unittest tests.test_new_releases tests.test_duanju_extended tests.test_device_pool_data_dir -v`

Expected: all tests pass and no secrets appear in output.

- [ ] **Step 8: Document public endpoints**

Add sanitized curl examples for `/series-metrics` and `/new-releases`, normalized response field tables, cursor rules, Shanghai-day semantics, and the first-load-silent notification behavior to `API_USAGE.md`.

- [ ] **Step 9: Record checkpoint**

Record endpoint tests and file list; do not commit.

---

### Task 3: Grouped Categories and Automatic Highest Definition

**Files:**
- Modify: `endpoints/duanju.py`
- Modify: `tests/test_duanju_extended.py`
- Modify: `API_USAGE.md`

**Interfaces:**
- Produces: `GET /api/duanju/categories?content_type=drama|manju` returning grouped selectors.
- Extends: `_pick_source(sources, definition)` with `definition == "auto"`.

- [ ] **Step 1: Write failing tests**

```python
def test_auto_definition_prefers_1080_then_720(self):
    sources = [source("720p"), source("1080p")]
    self.assertEqual(_pick_source(sources, "auto")["definition"], "1080p")
    self.assertEqual(_pick_source([source("720p")], "auto")["definition"], "720p")

def test_category_groups_keep_upstream_order(self):
    selector = {"inner_rows": [
        {"row_name": "时代背景", "items": [{"selector_item_id": "cate_4", "show_name": "校园"}]},
        {"row_name": "角色设定", "items": [{"selector_item_id": "cate_20", "show_name": "神豪"}]},
    ]}
    self.assertEqual(_category_groups(selector)[0]["name"], "时代背景")
```

- [ ] **Step 2: Run and verify RED**

Run: `python3 -m unittest tests.test_duanju_extended.DuanjuExtendedTests -v`

Expected: auto currently falls back to 720 and grouped helper is missing.

- [ ] **Step 3: Implement auto source ordering**

For `auto`, use `['1080p', '720p', '540p', '480p', '360p']`; for fixed values retain current “requested, then lower, then higher” order. Keep response header `X-Duanju-Definition` unchanged so the app records the actual result.

- [ ] **Step 4: Implement grouped categories endpoint**

Normalize selector rows to:

```json
{"groups":[{"id":"时代背景","name":"时代背景","items":[{"id":"cate_4","name":"校园"}]}]}
```

Preserve row and item order, drop blank IDs/names, de-duplicate IDs, and always prepend `综合` with `全部/女频/男频` when upstream supplies them. Use the existing signed bookmall/rank selector call and caches; do not parse the Reqable file at runtime.

- [ ] **Step 5: Run Python suites and live local API smoke check**

Run:

```bash
python3 -m unittest tests.test_duanju_extended tests.test_new_releases -v
```

Start `python3 -m uvicorn main:app --host 127.0.0.1 --port 18766` in a managed/background exec session. In a second command, request `/api/duanju/categories`, `/api/duanju/new-releases?type=all&limit=20`, and one `/api/duanju/series-metrics` using the returned ID **and numeric content type**. Print only response counts, normalized field names, and status; never print signing/device data. Stop the managed server after the smoke check.

- [ ] **Step 6: Update API docs and record checkpoint**

Document `definition=auto` and grouped categories, then record live result counts; do not commit.

---

### Task 4: Native Versioned Settings Persistence

**Files:**
- Create: `desktop/src-tauri/src/settings.rs`
- Modify: `desktop/src-tauri/src/lib.rs`

**Interfaces:**
- Produces Rust `AppSettings { version, save_dir, definition, notify_download_complete, notify_new_releases }`.
- Produces Tauri commands `get_settings`, `update_settings`, and settings-backed `choose_save_dir`.
- Consumed by frontend Task 6.

- [ ] **Step 1: Write failing Rust settings tests in `settings.rs`**

```rust
#[test]
fn missing_settings_use_defaults() {
    let path = unique_test_path("missing");
    let settings = load_settings(&path, PathBuf::from("/tmp/default"));
    assert_eq!(settings.definition, DefinitionPreference::Auto);
    assert!(settings.notify_download_complete);
    assert!(settings.notify_new_releases);
}

#[test]
fn invalid_definition_is_rejected() {
    let mut settings = AppSettings::default_for(PathBuf::from("/tmp/default"));
    assert!(settings.apply(UpdateSettings { definition: Some("4k".into()), ..Default::default() }).is_err());
}

#[test]
fn final_download_name_uses_actual_definition() {
    let path = download_destination(Path::new("/tmp/剧名"), "第 1 集", "1080p");
    assert!(path.ends_with("第 1 集_1080p.mp4"));
}
```

- [ ] **Step 2: Run and verify RED**

Run: `cargo test settings --manifest-path desktop/src-tauri/Cargo.toml`

Expected: module/types are missing.

- [ ] **Step 3: Implement settings model and atomic save**

Use serde camelCase fields and an enum serialized as `auto`, `1080p`, `720p`. `save_settings` must create the parent, write `settings.json.tmp`, call `sync_all`, and rename to `settings.json`. On corrupt/unsupported data, return defaults plus a warning string without deleting the damaged file.

- [ ] **Step 4: Replace `save_dir: Mutex<PathBuf>` with `settings: Mutex<AppSettings>`**

During `setup`, resolve `app.path().app_config_dir()/settings.json`, load defaults with `~/Downloads/红果下载`, create the selected directory, and store both settings path and settings in `AppState`.

- [ ] **Step 5: Add commands**

```rust
#[tauri::command]
fn get_settings(state: State<AppState>) -> AppResult<AppSettings>;

#[tauri::command]
fn update_settings(state: State<AppState>, patch: UpdateSettings) -> AppResult<AppSettings>;
```

`choose_save_dir` updates and persists `save_dir`; download execution snapshots the current save directory before entering blocking work. Keep `get_save_dir` temporarily as a compatibility wrapper until frontend migration is green.

Refactor `perform_download_episode` so it reads `X-Duanju-Definition` before choosing the final destination name. Stream into a collision-safe `.part` path, then atomically rename to a final filename built from the **actual** definition. Thus an `auto` request that resolves to 1080p produces `第 1 集_1080p.mp4`, while the returned result/path and queue record all agree on 1080p.

- [ ] **Step 6: Run native checks**

Run:

```bash
cargo fmt --check --manifest-path desktop/src-tauri/Cargo.toml
cargo test --manifest-path desktop/src-tauri/Cargo.toml
```

Expected: all Rust tests pass.

- [ ] **Step 7: Record checkpoint**

Record settings tests and commands; do not commit.

---

### Task 5: Notification Runtime Adapter

**Files:**
- Modify: `desktop/src-tauri/Cargo.toml`
- Modify: `desktop/src-tauri/src/lib.rs`
- Modify: `desktop/src-tauri/capabilities/default.json`
- Modify: `desktop/package.json`
- Modify: `desktop/package-lock.json`
- Create: `desktop/src/notifications.ts`
- Create: `desktop/src/notifications.test.ts`

**Interfaces:**
- Produces `NotificationAdapter` with `getStatus()` and `send(title, body)`.
- Produces `createTauriNotificationAdapter()` used by monitor/download hooks.

- [ ] **Step 1: Write failing adapter tests**

```ts
it("requests permission once and sends after it is granted", async () => {
  const runtime = fakeRuntime({ granted: false, requestResult: "granted" });
  const adapter = createNotificationAdapter(runtime);
  await adapter.send("发现新剧", "新增 2 部");
  await adapter.send("下载完成", "共 60 集");
  expect(runtime.requestPermission).toHaveBeenCalledTimes(1);
  expect(runtime.sendNotification).toHaveBeenCalledTimes(2);
});
```

- [ ] **Step 2: Run and verify RED**

Run: `npm test -- src/notifications.test.ts`

Expected: module missing.

- [ ] **Step 3: Install and register the matching Tauri 2 notification plugin**

Run:

```bash
cd desktop
npm install @tauri-apps/plugin-notification@2
```

Add `tauri-plugin-notification = "2"` to Cargo dependencies, `.plugin(tauri_plugin_notification::init())` to the builder, and `notification:default` to `capabilities/default.json`.

- [ ] **Step 4: Implement adapter**

```ts
export type NotificationAdapter = {
  getStatus(): Promise<"granted" | "denied" | "prompt">;
  send(title: string, body: string): Promise<boolean>;
};
```

Cache the permission attempt promise per adapter instance; a denied permission returns `false` and does not throw into application flows. Export a dependency-injectable factory for tests and a Tauri runtime factory for production.

- [ ] **Step 5: Run JS and Rust checks**

Run:

```bash
npm test -- src/notifications.test.ts
cargo check --manifest-path desktop/src-tauri/Cargo.toml
```

Expected: both pass.

- [ ] **Step 6: Record checkpoint**

Record dependency versions and test output; do not commit.

---

### Task 6: Frontend Types, API, Settings Hook, and Settings Page

**Files:**
- Modify: `desktop/src/types.ts`
- Modify: `desktop/src/api.ts`
- Create: `desktop/src/settings/useAppSettings.ts`
- Create: `desktop/src/settings/useAppSettings.test.tsx`
- Create: `desktop/src/settings/SettingsPage.tsx`
- Create: `desktop/src/settings/SettingsPage.test.tsx`

**Interfaces:**
- Produces `AppSettings`, `DefinitionPreference`, `SeriesMetrics`, `NewReleasePage`, and `CategoryGroup` types.
- Extends `SeriesItem` with `contentTypeCode: number` plus optional normalized metric fields.
- Produces `useAppSettings()` with `{settings, loading, warning, notificationPermission, update, chooseDirectory, openDirectory}`.

- [ ] **Step 1: Add failing hook tests**

```tsx
it("loads settings and rolls back a failed update", async () => {
  api.getSettings.mockResolvedValue(defaultSettings);
  api.updateSettings.mockRejectedValue(new Error("disk full"));
  const { result } = renderHook(() => useAppSettings(api));
  await waitFor(() => expect(result.current.settings?.definition).toBe("auto"));
  await act(() => result.current.update({ definition: "720p" }));
  expect(result.current.settings?.definition).toBe("auto");
  expect(result.current.warning).toContain("disk full");
});
```

- [ ] **Step 2: Run hook test and verify RED**

Run: `npm test -- src/settings/useAppSettings.test.tsx`

Expected: module missing.

- [ ] **Step 3: Add API functions and settings hook**

```ts
export type AppSettings = {
  version: 1;
  saveDir: string;
  definition: "auto" | "1080p" | "720p";
  notifyDownloadComplete: boolean;
  notifyNewReleases: boolean;
};

export type SeriesItem = {
  // existing fields remain unchanged
  contentTypeCode: number;
  onlineTime?: number;
  playCount?: number;
  hotCount?: number;
  collectCount?: number;
  likeCount?: number;
};

export const getSettings = () => invoke<AppSettings>("get_settings");
export const updateSettings = (patch: Partial<Omit<AppSettings, "version">>) =>
  invoke<AppSettings>("update_settings", { patch });
```

The hook serializes updates through a promise chain to prevent two toggles from overwriting each other and restores the previous settings on failure. It obtains `notificationPermission` from the notification adapter without requesting permission merely by opening Settings.

- [ ] **Step 4: Write failing SettingsPage interaction test**

Assert the page renders the current path, three resolution radio controls, two notification switches, calls `chooseDirectory`, displays warning text, and renders the current notification permission status/system-settings hint.

- [ ] **Step 5: Implement SettingsPage and run tests**

Run: `npm test -- src/settings/useAppSettings.test.tsx src/settings/SettingsPage.test.tsx`

Expected: all pass.

- [ ] **Step 6: Record checkpoint**

Record tests and exposed frontend interfaces; do not commit.

---

### Task 7: Exact 20-Item Group Paging and Grouped Homepage Categories

**Files:**
- Create: `desktop/src/feed/groupedPaging.ts`
- Create: `desktop/src/feed/groupedPaging.test.ts`
- Create: `desktop/src/components/CategoryFilter.tsx`
- Create: `desktop/src/components/CategoryFilter.test.tsx`
- Modify: `desktop/src/api.ts`
- Modify: `desktop/src/App.feed-search.test.tsx`
- Modify: `desktop/src/styles.css`

**Interfaces:**
- Produces `fillUniqueGroup<TPage>()` for home/search/rank.
- Produces `CategoryFilter` with `groups`, `selectedId`, `onSelect`.

- [ ] **Step 1: Write failing exact-group tests**

```ts
it("reveals exactly twenty new unique items while preserving overfetch buffer", async () => {
  const pages = [page(1, 6), page(7, 6), page(13, 6), page(19, 6), page(25, 6), page(31, 10)];
  const result1 = await fillUniqueGroup(emptyState(), () => Promise.resolve(pages.shift()!), 20);
  expect(result1.visible).toHaveLength(20);
  expect(result1.buffer).toHaveLength(4);
  const result2 = await fillUniqueGroup(result1.state, () => Promise.resolve(pages.shift()!), 20);
  expect(result2.visible).toHaveLength(40);
});
```

Add a second test where exact-title filtering discards most upstream items and keeps requesting until 20 exact matches or `hasMore=false`.

- [ ] **Step 2: Run and verify RED**

Run: `npm test -- src/feed/groupedPaging.test.ts`

Expected: module missing.

- [ ] **Step 3: Implement grouped paging**

The state contains `allItems`, `visibleCount`, opaque upstream page state, `hasMore`, and `loading`. Each call targets `visibleCount + 20`, de-duplicates by `bookId`, caps upstream requests at 10, and slices visible data without dropping over-fetched items.

- [ ] **Step 4: Write CategoryFilter failing test**

```tsx
it("switches groups and expands the long category row", () => {
  const view = render(<CategoryFilter groups={groups} selectedId="" onSelect={onSelect} />);
  fireEvent.click(view.getByRole("button", { name: "主题情节" }));
  expect(view.getByRole("button", { name: "打脸虐渣" })).toBeTruthy();
  fireEvent.click(view.getByRole("button", { name: /展开/ }));
  expect(view.getByRole("button", { name: "都市修仙" })).toBeTruthy();
});
```

- [ ] **Step 5: Implement grouped horizontal category UI**

Default to the first group, show at most 10 category chips while collapsed, keep the current selected category visible, and reset expansion when switching groups. Fetch groups with `/api/duanju/categories`.

- [ ] **Step 6: Change grid CSS to five columns and update feed tests**

Set `.poster-grid { grid-template-columns: repeat(5, minmax(0, 1fr)); }` for the supported desktop content area. Add `data-page-size="20"` or equivalent observable behavior so tests assert 20 initial and +20 on scroll for home, search, and rank; rank asserts `NO.20` then `NO.40`.

- [ ] **Step 7: Run focused tests**

Run:

```bash
npm test -- src/feed/groupedPaging.test.ts src/components/CategoryFilter.test.tsx src/App.feed-search.test.tsx
```

Expected: all pass.

- [ ] **Step 8: Record checkpoint**

Record card counts and test output; do not commit.

---

### Task 8: Series Metrics in Inspector and Resolution Snapshot in Queue

**Files:**
- Modify: `desktop/src/types.ts`
- Modify: `desktop/src/api.ts`
- Modify: `desktop/src/components/SeriesInspector.tsx`
- Modify: `desktop/src/components/SeriesInspector.test.tsx`
- Modify: `desktop/src/download/model.ts`
- Modify: `desktop/src/download/model.test.ts`
- Modify: `desktop/src/download/storage.ts`
- Modify: `desktop/src/preview.ts`

**Interfaces:**
- Consumes: `fetchSeriesMetrics`, `AppSettings.definition`.
- Changes: `enqueueEpisodes(state, series, episodes, definition, ...)` snapshots `auto|1080p|720p` into new queue items.

- [ ] **Step 1: Add failing inspector metric test**

Render a series with `onlineTime`, `playCount: 0`, `hotCount`, `collectCount: undefined`, and `likeCount`; assert the exact labels `上线时间`, `播放量 0`, `收藏量 —` appear.

- [ ] **Step 2: Add failing enqueue definition test**

```ts
const result = enqueueEpisodes(createInitialState(), series, episodes, "720p", idFactory, now);
expect(result.state.batches[0].items[0].definition).toBe("720p");
```

- [ ] **Step 3: Run and verify RED**

Run: `npm test -- src/components/SeriesInspector.test.tsx src/download/model.test.ts`

Expected: metrics absent and old enqueue signature ignores definition.

- [ ] **Step 4: Implement model/type/UI changes**

Add optional metric fields to `SeriesItem`; add a compact 2x3 metrics block in the inspector; format counts with Chinese `万/亿` units while preserving zero. `fetchSeriesMetrics` must pass `series.seriesId` together with `series.contentTypeCode`, and its stale-response guard must key both values. Replace hard-coded `1080p` copy with current preference/actual queue definition.

Change `enqueueEpisodes` so **new** items snapshot the current setting (`auto|1080p|720p`). When reading legacy queue records whose `definition` field is absent, retain the historical fallback `1080p`; never silently reinterpret old queued work as `auto`. Existing stored definitions remain unchanged.

- [ ] **Step 5: Run focused tests**

Run: `npm test -- src/components/SeriesInspector.test.tsx src/download/model.test.ts src/download/storage.test.ts`

Expected: all pass.

- [ ] **Step 6: Record checkpoint**

Record model migration behavior; do not commit.

---

### Task 9: New-Release Monitor Storage, Hook, Page, and Notifications

**Files:**
- Create: `desktop/src/monitor/storage.ts`
- Create: `desktop/src/monitor/storage.test.ts`
- Create: `desktop/src/monitor/useNewReleaseMonitor.ts`
- Create: `desktop/src/monitor/useNewReleaseMonitor.test.tsx`
- Create: `desktop/src/monitor/NewReleasesPage.tsx`
- Create: `desktop/src/monitor/NewReleasesPage.test.tsx`
- Modify: `desktop/src/api.ts`
- Modify: `desktop/src/styles.css`

**Interfaces:**
- Produces `useNewReleaseMonitor({api, storage, notifications, enabled})`.
- Produces page state `{type, items, loading, error, refreshedAt, unseenCount, setType, refresh, clearUnseen}`.

- [ ] **Step 1: Write failing persistence tests**

The storage key is `hongguo.new-releases.seen.v1`; test corrupted data falls back safely, records are scoped by Shanghai date and type, and old dates are pruned.

- [ ] **Step 2: Write failing hook timer/baseline tests**

```tsx
it("uses a silent baseline then sends one aggregate notification for later IDs", async () => {
  vi.useFakeTimers();
  api.fetchNewReleases
    .mockResolvedValueOnce(releasePage([release("a")]))
    .mockResolvedValueOnce(releasePage([release("a"), release("b"), release("c")]));
  const { result } = renderHook(() => useNewReleaseMonitor(deps));
  await waitFor(() => expect(api.fetchNewReleases).toHaveBeenCalledTimes(1));
  expect(notifications.send).not.toHaveBeenCalled();
  await act(() => vi.advanceTimersByTimeAsync(300_000));
  expect(notifications.send).toHaveBeenCalledWith("发现今日新剧", expect.stringContaining("2 部"));
  expect(result.current.unseenCount).toBe(2);
});
```

- [ ] **Step 3: Run and verify RED**

Run: `npm test -- src/monitor/storage.test.ts src/monitor/useNewReleaseMonitor.test.tsx`

Expected: modules missing.

- [ ] **Step 4: Implement storage and hook**

Keep the interval mounted while the app is open, serialize refresh calls to avoid overlap, reset paging on manual refresh/type change, and merge exactly 20 items per scroll group. Only notify when `settings.notifyNewReleases` is true; always update unseen count even when permission is denied.

- [ ] **Step 5: Write failing page test and implement five-column cards**

Test type filters, `HH:mm` Shanghai time, five metrics including zeros/missing, `立即刷新`, empty-today state, and scroll loading. The page receives hook state via props so rendering tests do not need real timers.

- [ ] **Step 6: Run focused monitor tests**

Run:

```bash
npm test -- src/monitor/storage.test.ts src/monitor/useNewReleaseMonitor.test.tsx src/monitor/NewReleasesPage.test.tsx
```

Expected: all pass.

- [ ] **Step 7: Record checkpoint**

Record timer and exactly-once notification assertions; do not commit.

---

### Task 10: Exactly-Once Download Batch Completion Notification

**Files:**
- Modify: `desktop/src/download/model.ts`
- Modify: `desktop/src/download/model.test.ts`
- Modify: `desktop/src/download/storage.ts`
- Modify: `desktop/src/download/storage.test.ts`
- Modify: `desktop/src/download/useDownloadManager.ts`
- Modify: `desktop/src/download/useDownloadManager.test.tsx`

**Interfaces:**
- Adds `completionNotifiedAt?: number` to `DownloadBatch`.
- Adds `mark-batch-notified` reducer action.
- Adds optional manager callback `onBatchCompleted(batch) -> Promise<void> | void`.

- [ ] **Step 1: Write failing reducer/storage tests**

Assert `mark-batch-notified` stamps one batch and survives serialization. When loading legacy data, batches already fully done receive `completionNotifiedAt = updatedAt` so upgrading the app does not notify old work.

- [ ] **Step 2: Write failing manager transition test**

Create a two-item batch with one done/one running; resolve the final download; assert callback once with title/count, re-render, and assert no second callback.

- [ ] **Step 3: Run and verify RED**

Run:

```bash
npm test -- src/download/model.test.ts src/download/storage.test.ts src/download/useDownloadManager.test.tsx
```

Expected: callback/action fields missing.

- [ ] **Step 4: Implement transition and persistence**

After each state commit, identify batches where all items are done and `completionNotifiedAt` is absent. Invoke callback, then immediately persist `mark-batch-notified` whether notification is sent, disabled, denied, or errors; notification failure must not block the queue.

The App callback sends one message only when `settings.notifyDownloadComplete` is true:

```ts
await notifications.send("下载完成", `《${batch.title}》共 ${batch.items.length} 集下载完成`);
```

- [ ] **Step 5: Run download tests**

Run the three focused suites above; expect all pass.

- [ ] **Step 6: Record checkpoint**

Record exactly-once evidence; do not commit.

---

### Task 11: App Composition, Navigation, Stale Requests, and Preview Data

**Files:**
- Modify: `desktop/src/components/AppRail.tsx`
- Modify: `desktop/src/components/AppRail.test.tsx`
- Modify: `desktop/src/App.tsx`
- Modify: `desktop/src/App.test.tsx`
- Modify: `desktop/src/App.data-races.test.tsx`
- Modify: `desktop/src/App.feed-search.test.tsx`
- Modify: `desktop/src/preview.ts`
- Modify: `desktop/src/styles.css`

**Interfaces:**
- Consumes Tasks 5–10.
- Produces final nav IDs: `discover`, `search`, `rank`, `monitor`, `queue`, `settings`.

- [ ] **Step 1: Extend failing AppRail test**

Assert `新剧监听` and `设置` buttons exist, badge `3` renders only on monitor, and clicking each emits the matching nav ID.

- [ ] **Step 2: Extend failing App integration test**

Mock settings/new-release APIs and notification adapter. Exercise:

```text
首页 -> 20 cards -> scroll -> 40 cards
搜索 -> exact mode -> 20-result group
榜单 -> NO.1..NO.20 -> scroll -> NO.40
新剧监听 -> five-column cards -> opening clears badge
设置 -> choose 720p -> enqueue -> queued item definition is 720p
```

- [ ] **Step 3: Run and verify RED**

Run:

```bash
npm test -- src/components/AppRail.test.tsx src/App.test.tsx src/App.data-races.test.tsx src/App.feed-search.test.tsx
```

Expected: new nav/pages absent and old load-more increments are not exactly 20.

- [ ] **Step 4: Compose focused pages/hooks in App**

Start independent health/settings/monitor promises together where possible. Catalog and series-metrics requests start together and use the existing request ID guard; a late metric response for a previous series must not overwrite the current selection.

Keep library scroll handling only for discover/search/rank; monitor uses its page handler. Opening monitor calls `clearUnseen`. Settings page receives the hook rather than duplicating local state.

- [ ] **Step 5: Update preview mode**

Provide 40 sanitized local series, metric samples including zero/missing, grouped categories, settings, and today-new-release items. Never reuse signed image URLs from the Reqable collection; use existing preview assets/URLs already in the project or empty-cover fallbacks.

- [ ] **Step 6: Complete CSS**

Verify 5 equal columns inside the existing library pane, rank badges, category chips, monitor 2x2 metric blocks, settings controls, nav badges, empty/error/loading states, and no horizontal scroll at the supported desktop minimum size.

- [ ] **Step 7: Run the full frontend suite**

Run: `npm test -- --run`

Expected: all suites pass with no console warnings.

- [ ] **Step 8: Record checkpoint**

Record all frontend test counts and main changed files; do not commit.

---

### Task 12: End-to-End Verification, Native QA, and Delivery

**Files:**
- Modify only if verification exposes a regression; each fix requires a new failing test first.
- Deliver: `desktop/src-tauri/target/release/bundle/macos/红果下载.app`
- Deliver: `desktop/src-tauri/target/release/bundle/dmg/红果下载_0.1.0_aarch64.dmg`

**Interfaces:**
- Consumes all earlier tasks.
- Produces fresh automated, browser, native, signing, and checksum evidence.

- [ ] **Step 1: Run all automated checks in parallel**

```bash
cd desktop && npm test -- --run
cd desktop && npm run build
cd desktop/src-tauri && cargo fmt --check
cd desktop/src-tauri && cargo test
cd ../.. && python3 -m unittest discover -s tests -v
```

All must exit 0. Do not infer success from partial suites.

- [ ] **Step 2: Run sanitized live API validation**

Start local API on an isolated port. Verify:

- categories return named groups and no raw credentials;
- metrics return all five normalized fields;
- new releases return only today in Shanghai and at most 20 items;
- typed filters use distinct candidate sources;
- `definition=auto` returns the actual selected definition.

If today has zero upstream releases, report that as valid live evidence and use deterministic fixtures for non-empty UI coverage.

- [ ] **Step 3: Browser QA**

Use the Browser plugin workflow against preview mode. Required target flows:

- page identity, meaningful DOM, no error overlay, clean console;
- home/search/rank each show 5 columns and 20 initial cards;
- scroll adds exactly 20 and rank reaches `NO.40`;
- category group/expand interaction;
- monitor type switch/manual refresh/empty and populated fixture states;
- settings resolution and notification switches;
- screenshot evidence for home categories, monitor cards, and settings.

- [ ] **Step 4: Native Tauri QA**

Build an isolated QA bundle ID. Verify folder chooser, persisted settings after relaunch, actual download definition snapshot, five-column layout, real metrics, 5-minute timer with a shortened test-only interval only in QA mode, new-release badge clearing, and both macOS notification types. Do not start a real multi-episode download merely to test notifications; use a one-item controlled QA task.

- [ ] **Step 5: Build formal package**

Run: `cd desktop && npm run tauri build`

- [ ] **Step 6: Verify artifact integrity**

```bash
codesign --verify --deep --strict --verbose=2 "desktop/src-tauri/target/release/bundle/macos/红果下载.app"
hdiutil verify "desktop/src-tauri/target/release/bundle/dmg/红果下载_0.1.0_aarch64.dmg"
shasum -a 256 "desktop/src-tauri/target/release/bundle/dmg/红果下载_0.1.0_aarch64.dmg"
```

Report that the build is ad-hoc signed and not Apple-notarized unless valid Apple credentials are configured during this run.

- [ ] **Step 7: Final scope/security audit**

Search the workspace for copied capture names, signature headers, bearer/cookie literals, device IDs from the capture, and temporary response dumps. Remove only artifacts created by this implementation. Confirm the provided Reqable file remains outside the project and unchanged.

- [ ] **Step 8: Record final checkpoint**

Report feature behavior, exact test totals, live-data boundary, browser/native evidence, artifact links, signing/notarization state, and SHA-256. State again that no Git commit/PR exists because the workspace is not a repository.
