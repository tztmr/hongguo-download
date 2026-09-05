# 红果下载 UI 与批量下载管理 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把现有红果桌面端升级为经过视觉重做、支持按剧分组、默认并发 5、可持久化恢复的批量下载管理器。

**Architecture:** TypeScript 领域模型负责所有任务状态、派生统计和公平调度；React Hook 负责持久化、Tauri 进度订阅与并发执行；现有 Rust `download_episode` 命令保持单集下载与解密职责。UI 分为内容浏览工作区和 B1 固定右侧详情的下载管理页，并提供仅用于浏览器验证的确定性预览模式。

**Tech Stack:** React 19、TypeScript 5.8、Vite 7、Vitest、Testing Library、Tauri 2、Rust、CSS。

**Spec:** `docs/superpowers/specs/2026-09-02-download-manager-redesign.md`

## Global Constraints

- 下载管理列表按剧分组，每部剧一行，右侧固定显示逐集详情。
- 全局并发默认 5，可调整为 1–10；不同剧共享额度并公平轮转。
- 暂停不取消当前单集；当前集完成后停止启动后续项。
- 删除任务和清理记录不能删除磁盘 MP4。
- 应用关闭期间不下载；重启前的 `running` 项恢复为 `queued` 并从头下载。
- 不增加 SQLite、HTTP Range 断点续传或后台常驻服务。
- 真实下载路径继续使用 Tauri `download_episode`，不得改成浏览器 Blob 下载。
- 当前目录不是 Git 仓库；所有“提交”检查点改为记录测试结果和检查实际文件差异，不执行 `git commit`。

## File Structure

- Create `desktop/src/download/model.ts`: 下载领域类型、reducer、派生统计、重复过滤、公平调度。
- Create `desktop/src/download/model.test.ts`: 领域行为与调度红绿测试。
- Create `desktop/src/download/storage.ts`: 版本化 localStorage 序列化、损坏备份和恢复。
- Create `desktop/src/download/storage.test.ts`: 恢复、限幅和损坏数据测试。
- Create `desktop/src/download/useDownloadManager.ts`: React 状态协调、并发 Promise、进度事件和持久化。
- Create `desktop/src/download/useDownloadManager.test.tsx`: Hook 的并发、暂停、完成和失败测试。
- Create `desktop/src/components/icons.tsx`: 本地 SVG 图标集合。
- Create `desktop/src/components/Cover.tsx`: 封面与失败回退。
- Create `desktop/src/components/AppRail.tsx`: 一级导航与下载徽标。
- Create `desktop/src/components/SeriesInspector.tsx`: 逐集勾选、全选、反选和范围选择。
- Create `desktop/src/components/SeriesInspector.test.tsx`: 批量选集交互测试。
- Create `desktop/src/components/DownloadManagerPage.tsx`: B1 下载中心和逐集详情。
- Create `desktop/src/components/DownloadManagerPage.test.tsx`: 分组展示与任务操作测试。
- Create `desktop/src/preview.ts`: 浏览器预览数据、下载适配器和可重复进度模拟。
- Create `desktop/src/preview.test.ts`: 预览统计与确定性数据测试。
- Modify `desktop/src/App.tsx`: 接入领域 Hook、拆分页面、保留发现/搜索/榜单 API 流程。
- Modify `desktop/src/types.ts`: 移除旧扁平 `DownloadTask`，保留内容 API 类型。
- Modify `desktop/src/styles.css`: 完整实现已确认的炭灰红色 UI、B1 布局和窄窗口覆盖详情。
- Modify `desktop/src/api.ts`: 导出下载适配器所需类型，保持真实 Tauri 调用。
- Modify `desktop/package.json`: 添加测试脚本和测试依赖。
- Modify `desktop/vite.config.ts`: 添加 Vitest/jsdom 配置。

---

### Task 1: 下载领域模型与测试基础

**Files:**
- Create: `desktop/src/download/model.ts`
- Create: `desktop/src/download/model.test.ts`
- Modify: `desktop/package.json`
- Modify: `desktop/vite.config.ts`

**Interfaces:**
- Produces: `DownloadItem`, `DownloadBatch`, `DownloadManagerState`, `DownloadStats`, `DownloadAction`。
- Produces: `createInitialState()`, `enqueueEpisodes()`, `downloadReducer()`, `deriveBatchStatus()`, `getDownloadStats()`, `selectLaunchableItems()`。
- Consumes: `SeriesItem`、`EpisodeItem` from `desktop/src/types.ts`。

- [ ] **Step 1: 添加测试运行器配置**

在 `package.json` 中加入：

```json
"test": "vitest run",
"test:watch": "vitest"
```

开发依赖加入 `vitest`、`jsdom`、`@testing-library/react`。`vite.config.ts` 从 `vitest/config` 导入 `defineConfig` 并配置：

```ts
test: {
  environment: "jsdom",
  globals: true,
  setupFiles: [],
}
```

- [ ] **Step 2: 写领域模型失败测试**

测试必须先覆盖默认并发、按剧合并和重复过滤：

```ts
it("creates grouped batches and skips duplicate episodes", () => {
  const initial = createInitialState();
  expect(initial.concurrency).toBe(5);

  const first = enqueueEpisodes(initial, series, episodes.slice(0, 2), fixedIds, 1000);
  const second = enqueueEpisodes(first.state, series, episodes.slice(1, 3), fixedIds, 2000);

  expect(second.state.batches).toHaveLength(1);
  expect(second.state.batches[0].items.map((item) => item.itemId)).toEqual(["e1", "e2", "e3"]);
  expect(second.added).toBe(1);
  expect(second.skipped).toBe(1);
});
```

再分别覆盖：并发限幅 1–10、全局/剧级暂停、单集和剧级重试、删除等待项、完成任务清理、`removeWhenIdle`。

- [ ] **Step 3: 运行测试并确认正确失败**

Run: `npm test -- src/download/model.test.ts`

Expected: FAIL，原因是 `./model` 或其导出函数尚不存在，而不是测试语法错误。

- [ ] **Step 4: 实现最小领域模型**

核心签名固定为：

```ts
export function createInitialState(): DownloadManagerState;

export function enqueueEpisodes(
  state: DownloadManagerState,
  series: SeriesItem,
  episodes: EpisodeItem[],
  idFactory?: () => string,
  now?: number,
): { state: DownloadManagerState; added: number; skipped: number };

export function downloadReducer(
  state: DownloadManagerState,
  action: DownloadAction,
): DownloadManagerState;

export function selectLaunchableItems(
  state: DownloadManagerState,
  activeIds: ReadonlySet<string>,
  limit: number,
  cursor: number,
): { items: Array<{ batchId: string; item: DownloadItem }>; cursor: number };
```

选择器按批次轮转，每轮每部剧最多选择一集，直到填满 `limit` 或无可运行项。

- [ ] **Step 5: 运行领域测试并确认通过**

Run: `npm test -- src/download/model.test.ts`

Expected: 所有领域测试 PASS，无未处理 Promise 或控制台错误。

- [ ] **Step 6: 检查点**

运行 `npm run build`，记录测试和构建退出码；检查只新增测试基础和领域模型相关文件。

---

### Task 2: 版本化持久化与恢复

**Files:**
- Create: `desktop/src/download/storage.ts`
- Create: `desktop/src/download/storage.test.ts`

**Interfaces:**
- Consumes: `DownloadManagerState`, `createInitialState()` from Task 1。
- Produces: `DOWNLOAD_STORAGE_KEY`, `loadDownloadState(storage)`, `saveDownloadState(storage, state)`。

- [ ] **Step 1: 写恢复失败测试**

```ts
it("restores running items as fresh queued items", () => {
  const storage = memoryStorage({
    [DOWNLOAD_STORAGE_KEY]: JSON.stringify(runningFixture),
  });
  const result = loadDownloadState(storage);
  const item = result.state.batches[0].items[0];

  expect(item.status).toBe("queued");
  expect(item.percent).toBe(0);
  expect(item.received).toBe(0);
  expect(result.warning).toBeUndefined();
});
```

另写测试确认：完成项路径保留、并发 0/20 被限幅为 1/10、损坏 JSON 保存到 `hongguo.downloads.corrupt.<timestamp>` 并返回空状态与中文警告。

- [ ] **Step 2: 运行测试确认失败**

Run: `npm test -- src/download/storage.test.ts`

Expected: FAIL，原因是存储模块尚不存在。

- [ ] **Step 3: 实现持久化**

```ts
export const DOWNLOAD_STORAGE_KEY = "hongguo.downloads.v1";

export function loadDownloadState(storage: Storage): {
  state: DownloadManagerState;
  warning?: string;
};

export function saveDownloadState(storage: Storage, state: DownloadManagerState): void;
```

加载时只接受 `version === 1`、数组批次和合法单集状态；所有 `running` 统一恢复为 `queued`，并清空瞬时进度。

- [ ] **Step 4: 运行存储与领域测试**

Run: `npm test -- src/download/storage.test.ts src/download/model.test.ts`

Expected: 全部 PASS。

- [ ] **Step 5: 检查点**

运行 `npm run build`，确认无 `Storage` 类型或严格模式错误。

---

### Task 3: React 下载协调器

**Files:**
- Create: `desktop/src/download/useDownloadManager.ts`
- Create: `desktop/src/download/useDownloadManager.test.tsx`
- Modify: `desktop/src/api.ts`

**Interfaces:**
- Consumes: Task 1 reducer/selector、Task 2 storage、真实 `downloadEpisode()`。
- Produces: `DownloadAdapter`, `DownloadProgress`, `useDownloadManager(options)`。
- Produces Hook API: `state`, `stats`, `enqueue`, `pauseAll`, `resumeAll`, `setConcurrency`, `pauseBatch`, `resumeBatch`, `retryItem`, `retryBatch`, `removeItem`, `removeBatch`, `clearCompleted`, `updateProgress`。

- [ ] **Step 1: 写 Hook 并发失败测试**

使用 deferred Promise 的假下载适配器：

```tsx
it("starts five downloads by default and fills freed slots", async () => {
  const adapter = createDeferredAdapter();
  const { result } = renderHook(() => useDownloadManager({ adapter, storage }));
  act(() => result.current.enqueue(series, sixEpisodes));

  await waitFor(() => expect(adapter.startedIds()).toHaveLength(5));
  adapter.resolve(adapter.startedIds()[0]);
  await waitFor(() => expect(adapter.startedIds()).toHaveLength(6));
});
```

另测：全局暂停后运行项完成但第六项不启动；并发调到 1 不取消现有 Promise；reject 只把对应项设为 `error`；进度事件按 `taskId` 更新正确单集；卸载后不继续 setState。

- [ ] **Step 2: 运行 Hook 测试确认失败**

Run: `npm test -- src/download/useDownloadManager.test.tsx`

Expected: FAIL，原因是 Hook 尚不存在。

- [ ] **Step 3: 实现协调器**

适配器接口固定为：

```ts
export type DownloadAdapter = {
  download(args: {
    taskId: string;
    itemId: string;
    title: string;
    episodeTitle: string;
    definition: string;
  }): Promise<{ taskId: string; path: string; definition: string; bytes: number }>;
  subscribeProgress(listener: (progress: DownloadProgress) => void): Promise<() => void>;
};
```

Hook 使用 `activeRef: Map<string, Promise<void>>` 防止重复启动，使用 `cursorRef` 保存公平轮转游标；关键状态立即保存，纯进度更新使用 500ms 节流。

- [ ] **Step 4: 运行 Hook 全套测试**

Run: `npm test -- src/download/useDownloadManager.test.tsx src/download/model.test.ts src/download/storage.test.ts`

Expected: 全部 PASS，测试退出时无 act 警告。

- [ ] **Step 5: 检查点**

运行 `npm run build`，确认真实 Tauri 适配器的参数和返回值与 Rust 命令一致。

---

### Task 4: 批量选集组件

**Files:**
- Create: `desktop/src/components/icons.tsx`
- Create: `desktop/src/components/Cover.tsx`
- Create: `desktop/src/components/SeriesInspector.tsx`
- Create: `desktop/src/components/SeriesInspector.test.tsx`

**Interfaces:**
- Consumes: `SeriesItem`, `EpisodeItem`。
- Produces: `SeriesInspector` props `series`, `episodes`, `selectedIds`, `loading`, `onSelectionChange`, `onEnqueue`。

- [ ] **Step 1: 写选集交互失败测试**

```tsx
it("selects an inclusive episode range and enqueues the selected count", () => {
  const onSelectionChange = vi.fn();
  const onEnqueue = vi.fn();
  const view = render(
    <SeriesInspector
      series={series}
      episodes={episodes}
      selectedIds={[]}
      loading={false}
      onSelectionChange={onSelectionChange}
      onEnqueue={onEnqueue}
    />,
  );

  fireEvent.change(view.getByLabelText("起始集"), { target: { value: "2" } });
  fireEvent.change(view.getByLabelText("结束集"), { target: { value: "4" } });
  fireEvent.click(view.getByRole("button", { name: "选择范围" }));
  expect(onSelectionChange).toHaveBeenCalledWith(["e2", "e3", "e4"]);
});
```

另测：全选、反选、0 集禁用主按钮、主按钮文案包含已选数量。

- [ ] **Step 2: 运行测试确认失败**

Run: `npm test -- src/components/SeriesInspector.test.tsx`

Expected: FAIL，组件尚不存在。

- [ ] **Step 3: 实现组件和本地图标**

使用语义化按钮、checkbox 和 label；图标全部为本地 SVG React 组件，不引入图标 CDN 或 emoji。

- [ ] **Step 4: 运行组件测试**

Run: `npm test -- src/components/SeriesInspector.test.tsx`

Expected: 全部 PASS。

- [ ] **Step 5: 检查点**

运行 `npm run build`，检查组件未直接依赖 Tauri 或下载协调器。

---

### Task 5: B1 下载管理页面

**Files:**
- Create: `desktop/src/components/AppRail.tsx`
- Create: `desktop/src/components/DownloadManagerPage.tsx`
- Create: `desktop/src/components/DownloadManagerPage.test.tsx`

**Interfaces:**
- Consumes: Task 1 状态/统计、Task 3 Hook 动作、Task 4 `Cover` 和图标。
- Produces: `DownloadManagerPage` 和 `AppRail`。

- [ ] **Step 1: 写页面失败测试**

```tsx
it("renders one row per series and shows selected episode details", () => {
  const view = render(<DownloadManagerPage manager={managerFixture} saveDir="/Downloads/红果下载" />);
  expect(view.getAllByTestId("download-batch-row")).toHaveLength(2);
  expect(view.getByText("天下第一纨绔")).toBeTruthy();
  fireEvent.click(view.getByText("女子爱财，取之有道"));
  expect(view.getByText("第 21 集")).toBeTruthy();
});
```

另测：四个统计值、并发下拉为 1–10 且默认选中 5、暂停/继续、重试失败、清理已完成、空状态、窄屏详情返回按钮。

- [ ] **Step 2: 运行页面测试确认失败**

Run: `npm test -- src/components/DownloadManagerPage.test.tsx`

Expected: FAIL，页面组件尚不存在。

- [ ] **Step 3: 实现 B1 页面**

顶部顺序固定为标题/保存目录、全局操作、四张统计卡；主体固定为左侧剧级列表和右侧逐集详情。删除动作只调用记录移除回调，页面不暴露删除文件按钮。

- [ ] **Step 4: 运行页面测试**

Run: `npm test -- src/components/DownloadManagerPage.test.tsx`

Expected: 全部 PASS。

- [ ] **Step 5: 检查点**

运行 `npm run build`，确认页面没有重复实现状态派生逻辑。

---

### Task 6: 应用集成、预览模式与视觉重做

**Files:**
- Create: `desktop/src/preview.ts`
- Modify: `desktop/src/App.tsx`
- Modify: `desktop/src/types.ts`
- Modify: `desktop/src/styles.css`

**Interfaces:**
- Consumes: Tasks 1–5 全部接口，现有发现/搜索/榜单/目录 API。
- Produces: `?preview=downloads` 和 `?preview=library` 两个不调用 Tauri 的确定性浏览器状态。

- [ ] **Step 1: 写预览适配器失败测试**

把预览数据和适配器设计为普通导出，并在 `preview.test.ts` 断言：

```ts
it("provides grouped running, queued, done and error states for visual QA", () => {
  const state = createPreviewDownloadState();
  expect(getDownloadStats(state)).toEqual({ running: 5, queued: 16, done: 142, error: 2 });
  expect(state.batches.length).toBeGreaterThanOrEqual(3);
});
```

- [ ] **Step 2: 运行预览测试确认失败**

Run: `npm test -- src/preview.test.ts`

Expected: FAIL，预览模块尚不存在。

- [ ] **Step 3: 实现预览与 App 集成**

`App.tsx` 不再保存旧扁平 `tasks`。真实模式使用 Tauri 适配器，预览模式使用内存存储和模拟适配器；发现/搜索/榜单原 API 流程保持不变。加入队列后显示“已加入 N 集，跳过 M 个重复项”的 toast。

- [ ] **Step 4: 实现完整 CSS**

按已确认设计实现：炭灰背景、红色重点、72–80px 导航、内容卡片、右侧检查器、统计卡、剧级任务行和固定详情。至少包含：

```css
.download-workspace { grid-template-columns: minmax(420px, 1.55fr) minmax(300px, .85fr); }
@media (max-width: 980px) { .download-detail { position: absolute; inset: 0; } }
```

所有可交互元素必须有 hover、focus-visible、disabled 状态；任务名使用省略号，滚动区域不得嵌套造成页面锁死。

- [ ] **Step 5: 运行全部测试和构建**

Run: `npm test`

Run: `npm run build`

Expected: 两条命令都退出 0。

- [ ] **Step 6: 检查点**

检查 `App.tsx` 不再包含旧 `runningRef` 单任务泵和底部完整队列；检查 `types.ts` 不再导出旧 `DownloadTask`。

---

### Task 7: Rust、浏览器与真实下载验证

**Files:**
- Verify: `desktop/src-tauri/src/lib.rs`
- Verify: `desktop/dist/`
- Temporary screenshots: `/tmp/hongguo-download-manager-*.png`

**Interfaces:**
- Consumes: 完整实现。
- Produces: 可复核的自动化、构建、渲染和运行时验证证据。

- [ ] **Step 1: 运行完整前端测试和生产构建**

Run: `npm test`

Run: `npm run build`

Expected: 0 failed tests，构建退出 0。

- [ ] **Step 2: 运行 Rust 检查**

Run: `cargo check`

Working directory: `desktop/src-tauri`

Expected: 退出 0；现有 `download_episode` 命令和进度事件类型通过编译。

- [ ] **Step 3: 启动浏览器预览**

Run: `npm run dev -- --host 127.0.0.1`

Target flow: `?preview=library -> 选择范围并加入队列 -> 下载管理 -> 选择剧级任务 -> 右侧逐集详情更新`。

- [ ] **Step 4: 使用 Browser 插件执行渲染检查**

验证页面 URL/标题、DOM 非空、无 Vite 错误覆盖层、控制台无相关 error/warn。桌面视口至少 1440×900，窄窗口至少 900×760；分别保存截图到 `/tmp`。

- [ ] **Step 5: 验证目标交互**

在预览中完成范围选择、加入队列、进入下载管理、调整并发到 7、暂停/继续、切换任务并检查右侧详情；每个动作后验证可见状态变化。

- [ ] **Step 6: 尝试真实 Tauri 下载闭环**

运行桌面开发应用，在本地 API 可用时下载一集，确认进度从 `running` 到 `done`，最终路径可定位。如果上游接口、设备池或网络不可用，保存具体错误并把真实下载层标记为未验证，不能用预览结果替代。

- [ ] **Step 7: 最终范围复核**

逐项对照规格的完成标准，检查测试、构建、桌面/窄屏截图和真实下载边界。输出实际变更文件、命令退出码、截图及剩余风险。
