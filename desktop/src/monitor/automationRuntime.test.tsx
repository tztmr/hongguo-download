import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { AutomationPage } from "./AutomationPage";
import type { AutomationSnapshot } from "./automationRuntime";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => false }));
const storageKey = "hongguo.automation.settings-draft.v1";
const snapshot = (): AutomationSnapshot => ({
  config: { channel: "channel-fixture", privacy: "private", title: "后台 {剧名}", metadataVersion: 3, scope: "today" },
  mode: "stopped", jobs: [], logs: [], lastScan: 0, nextScan: 0, warning: "", keyStatus: { text: true, image: false },
});
let state: AutomationSnapshot;
let storage: Map<string, string>;
beforeEach(() => {
  state = snapshot(); storage = new Map();
  vi.stubGlobal("localStorage", { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value) });
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const payload = args as Record<string, unknown> | undefined;
    if (command === "save_automation_settings") state = { ...state, config: payload?.config as Record<string, unknown> };
    if (command === "start_automation") state = { ...state, mode: "running" };
    if (command === "control_automation") state = { ...state, mode: payload?.action === "pause" ? "paused" : payload?.action === "stop" ? "stopped" : "running" };
    return state as never;
  });
});
afterEach(() => { cleanup(); vi.useRealTimers(); vi.unstubAllGlobals(); vi.mocked(invoke).mockReset(); });
const page = () => render(<AutomationPage saveDir="/fixture/downloads" runtimeEnabled />);
const button = (view: ReturnType<typeof render>, name: string) => view.getByRole("button", { name }) as HTMLButtonElement;
// Save exists in both header and footer.
function save(view: ReturnType<typeof render>) { fireEvent.click(view.getAllByRole("button", { name: "保存设置" })[0]); }
async function loaded(view: ReturnType<typeof render>) { await waitFor(() => expect((view.getAllByRole("button", { name: "保存设置" })[0] as HTMLButtonElement).disabled).toBe(false)); }

describe("native automation boundaries", () => {
  it("distinguishes completed downloads from ongoing audio preparation and scheduled retry", async () => {
    state.mode = "running";
    state.jobs = [{ id: "audio", title: "预处理剧目", bookId: "book", stage: "separate", status: "pending",
      message: "音频预处理失败：磁盘空间不足", episodeDone: 152, episodeTotal: 152,
      progress: 6, updatedAt: 1, attempts: 1, retryAt: Math.floor(Date.now() / 1000) + 30 } as typeof state.jobs[number]];
    const view = page(); await loaded(view);
    expect(view.getByText("分离背景音乐", { selector: "b" })).toBeTruthy();
    expect(view.getByText(/下载已完成 152\/152 集/)).toBeTruthy();
    expect(view.getByText(/阶段进度 6%/)).toBeTruthy();
    expect(view.getByText(/第 1 次重试/)).toBeTruthy();
  });
  it("shows automatic cooldown recovery without requiring a retry click", async () => {
    state.mode = "running";
    state.jobs = [{ id: "cooldown", title: "等待恢复剧目", bookId: "book", stage: "separate", status: "observing",
      message: "音频预处理失败", episodeDone: 152, episodeTotal: 152, progress: 0, updatedAt: 1,
      attempts: 4, retryAt: Math.floor(Date.now() / 1000) + 900 }];
    const view = page(); await loaded(view);
    expect(view.getByText(/等待自动恢复/)).toBeTruthy();
    expect(view.queryByRole("button", { name: "重试此任务" })).toBeNull();
  });
  it("prefers saved backend config and starts only after saved target confirmation, without a config argument", async () => {
    storage.set(storageKey, JSON.stringify({ title: "旧本地模板" }));
    const view = page(); await loaded(view);
    fireEvent.click(view.getByRole("button", { name: /YouTube 上传/ }));
    expect((view.getByLabelText("YouTube 标题") as HTMLInputElement).value).toBe("后台 {剧名}");
    fireEvent.change(view.getByLabelText("YouTube 标题"), { target: { value: "修改 {剧名}" } });
    expect(button(view, "启动 24 小时自动任务").disabled).toBe(true);
    save(view);
    await waitFor(() => expect(button(view, "启动 24 小时自动任务").disabled).toBe(false));
    fireEvent.click(button(view, "启动 24 小时自动任务"));
    expect(view.getByLabelText("确认自动任务启动").textContent).toContain("channel-fixture；上传可见性：私享");
    expect(vi.mocked(invoke).mock.calls.some(([name]) => name === "start_automation")).toBe(false);
    fireEvent.click(button(view, "确认启动"));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("start_automation"));
    expect(vi.mocked(invoke).mock.calls.find(([name]) => name === "start_automation")?.length).toBe(1);
    expect(state.config?.title).toBe("修改 {剧名}");
  });
  it("migrates a local draft on save and sends only explicitly edited keys separately", async () => {
    state.config = null;
    storage.set(storageKey, JSON.stringify({ title: "迁移 {剧名}", textKey: "legacy-must-not-copy" }));
    const view = page(); await loaded(view);
    expect(button(view, "启动 24 小时自动任务").disabled).toBe(true);
    save(view);
    await waitFor(() => expect(state.config?.title).toBe("迁移 {剧名}"));
    const firstSave = vi.mocked(invoke).mock.calls.find(([name]) => name === "save_automation_settings")!;
    expect(firstSave[1]).not.toHaveProperty("secrets");
    expect((firstSave[1] as Record<string, unknown>)?.config).not.toHaveProperty("textKey");
    fireEvent.click(view.getByRole("button", { name: /AI 文案与封面/ }));
    const textInput = view.getByLabelText("文字服务 API Key", { exact: false });
    expect((textInput as HTMLInputElement).value).toBe("");
    fireEvent.change(textInput, { target: { value: "explicit-fixture-key" } });
    save(view);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("save_automation_settings", expect.objectContaining({ secrets: { text: "explicit-fixture-key" } })));
    expect(JSON.stringify(state.config)).not.toContain("explicit-fixture-key");
    expect(storage.get(storageKey)).not.toContain("explicit-fixture-key");
    await loaded(view);
    fireEvent.change(textInput, { target: { value: "" } });
    save(view);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("save_automation_settings", expect.objectContaining({ secrets: { text: "" } })));
  });
  it("deletes a stored key explicitly without requiring its plaintext, and leaves the other key untouched", async () => {
    const view = page(); await loaded(view);
    fireEvent.click(view.getByRole("button", { name: /AI 文案与封面/ }));
    expect((view.getByLabelText("文字服务 API Key", { exact: false }) as HTMLInputElement).value).toBe("");
    expect(vi.mocked(invoke).mock.calls.some(([name]) => name === "save_automation_settings")).toBe(false);
    fireEvent.click(button(view, "删除后台文字 Key（保存后生效）"));
    save(view);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("save_automation_settings", expect.objectContaining({ secrets: { text: "" } })));
  });
  it("controls execution and routes uncertain and failed tasks by stable job ID", async () => {
    state.mode = "running";
    state.jobs = [{ id: "review-id", title: "需核对剧目", bookId: "book", stage: "查重", status: "review", message: "标题相似", episodeDone: 0, episodeTotal: 10, updatedAt: 1, progress: 0 },
      { id: "retry-id", title: "失败剧目", bookId: "other", stage: "上传", status: "failed", message: "上传失败保留文件", episodeDone: 10, episodeTotal: 10, updatedAt: 1, progress: 70 }];
    const view = page(); await loaded(view);
    fireEvent.click(button(view, "暂停"));
    await waitFor(() => expect(button(view, "继续运行").disabled).toBe(false));
    fireEvent.click(button(view, "继续运行"));
    await waitFor(() => expect(button(view, "立即扫描").disabled).toBe(false));
    fireEvent.click(button(view, "立即扫描")); await loaded(view);
    fireEvent.click(button(view, "确认继续处理")); await loaded(view);
    fireEvent.click(button(view, "跳过此任务")); await loaded(view);
    fireEvent.click(button(view, "重试此任务")); await loaded(view);
    fireEvent.click(button(view, "停止")); await loaded(view);
    for (const action of ["pause", "resume", "scan", "stop"]) expect(invoke).toHaveBeenCalledWith("control_automation", { action });
    for (const action of ["continue", "skip"]) expect(invoke).toHaveBeenCalledWith("review_automation_job", { jobId: "review-id", action });
    expect(invoke).toHaveBeenCalledWith("review_automation_job", { jobId: "retry-id", action: "retry" });
  });
  it("reports backend errors and never enables starting from an unavailable snapshot", async () => {
    vi.mocked(invoke).mockRejectedValue({ message: "状态文件读取失败" });
    const view = page();
    await waitFor(() => expect(view.getByRole("alert").textContent).toBe("状态文件读取失败"));
    expect(button(view, "启动 24 小时自动任务").disabled).toBe(true);
    expect(button(view, "停止").disabled).toBe(true);
  });
  it("retains unsaved edits after a failed save", async () => {
    const view = page(); await loaded(view);
    vi.mocked(invoke).mockRejectedValueOnce(new Error("凭据库不可用"));
    fireEvent.change(view.getByLabelText("包含关键词", { exact: false }), { target: { value: "保留草稿" } });
    save(view);
    await waitFor(() => expect(view.getByRole("alert").textContent).toBe("凭据库不可用"));
    expect(button(view, "启动 24 小时自动任务").disabled).toBe(true);
    expect((view.getByLabelText("包含关键词", { exact: false }) as HTMLInputElement).value).toBe("保留草稿");
  });
  it("polls native progress without overwriting edits, and unmounting never stops the backend", async () => {
    vi.useFakeTimers();
    const view = page(); await act(async () => {});
    fireEvent.change(view.getByLabelText("包含关键词", { exact: false }), { target: { value: "未保存" } });
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "get_automation_snapshot")).toHaveLength(2);
    expect((view.getByLabelText("包含关键词", { exact: false }) as HTMLInputElement).value).toBe("未保存");
    view.unmount();
    await act(async () => { await vi.advanceTimersByTimeAsync(4000); });
    expect(vi.mocked(invoke).mock.calls).toHaveLength(2);
  });
  it("does not call the native runtime in browser preview", async () => {
    const view = render(<AutomationPage saveDir="/fixture" runtimeEnabled={false} />);
    expect(button(view, "启动 24 小时自动任务").disabled).toBe(true);
    fireEvent.click(view.getAllByRole("button", { name: "保存设置草稿" })[0]);
    expect(invoke).not.toHaveBeenCalled();
    expect(storage.has(storageKey)).toBe(true);
  });
});
