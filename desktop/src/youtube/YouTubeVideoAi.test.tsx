import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { YouTubeManagement } from "./YouTubeManagement";
import type { ManagedVideo, ManagementCommands, VideoAiDraft } from "./managementCommands";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
afterEach(cleanup);
const video: ManagedVideo = {
  id: "normal00001", etag: "rev1", title: "原剧名", description: "原剧情与链接", privacyStatus: "private", thumbnailUrl: "", publishedAt: "",
  videoFormat: "standard", durationSeconds: 600, restriction: { kind: "noneReported", reason: "", allowedRegions: null, blockedRegions: [] },
};
const short: ManagedVideo = { ...video, id: "shorts00001", title: "首集 Shorts", privacyStatus: "unlisted", videoFormat: "shorts" };
const image = "data:image/png;base64,c3ludGhldGljLXRlc3QtZml4dHVyZQ==";
function commands(items = [video, short]): ManagementCommands {
  let remote = items.map(row => ({ ...row }));
  return {
    list: vi.fn().mockResolvedValue({ items, nextPageToken: null }),
    detail: vi.fn(async (_channel, id) => remote.find(row => row.id === id)!),
    lookup: vi.fn(), deleteVideo: vi.fn(), makeBlockedVideoPrivate: vi.fn(), setPrivacy: vi.fn(),
    update: vi.fn(async request => {
      const updated = { ...remote.find(row => row.id === request.videoId)!, ...request, id: request.videoId, etag: "after-text" };
      remote = remote.map(row => row.id === updated.id ? updated : row); return updated;
    }),
    generateAi: vi.fn(async (_channel, id, _etag, kind) => ({ video: remote.find(row => row.id === id)!, title: kind === "text" ? `新标题 ${id}` : null, description: kind === "text" ? "新剧情与链接" : null, image: kind === "cover" ? image : null, model: "test-model" })),
    generatedThumbnail: vi.fn(async (_channel, id) => { remote = remote.map(row => row.id === id ? { ...row, etag: "after-cover", thumbnailUrl: image } : row); }),
    thumbnail: vi.fn(), playlists: vi.fn(), membership: vi.fn(), createPlaylist: vi.fn(),
  };
}
function mount(api: ManagementCommands) { return render(<YouTubeManagement channelId="c1" channelTitle="测试频道" commands={api} />); }
async function openBatch() {
  await waitFor(() => expect(screen.getByRole("checkbox", { name: "全选当前筛选结果" })).toHaveProperty("disabled", false));
  fireEvent.click(await screen.findByRole("checkbox", { name: "全选当前筛选结果" }));
  fireEvent.click(screen.getByRole("button", { name: "AI 一键优化（2）" }));
  return screen.getByRole("dialog", { name: "AI 一键优化视频" });
}
async function generate() {
  fireEvent.click(screen.getByRole("button", { name: "一键生成" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "生成未完成项" })).toHaveProperty("disabled", true));
}

describe("AI edits to channel videos", () => {
  it("previews both formats before writing, retains visibility, and uses the saved text revision for cover upload", async () => {
    const api = commands(); mount(api); await openBatch();
    expect(api.generateAi).not.toHaveBeenCalled();
    await generate();
    expect(vi.mocked(api.generateAi).mock.calls).toEqual([
      ["c1", video.id, "rev1", "text"], ["c1", video.id, "rev1", "cover"],
      ["c1", short.id, "rev1", "text"], ["c1", short.id, "rev1", "cover"],
    ]);
    expect(api.update).not.toHaveBeenCalled(); expect(api.generatedThumbnail).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText(`AI 标题 ${video.id}`), { target: { value: "人工核对后的标题" } });
    fireEvent.click(screen.getByRole("button", { name: "一键同步到 YouTube" }));
    await screen.findByText("所选内容已全部同步");
    expect(vi.mocked(api.update).mock.calls.map(([request]) => request.privacyStatus)).toEqual(["private", "unlisted"]);
    expect(vi.mocked(api.update).mock.calls[0][0].title).toBe("人工核对后的标题");
    expect(vi.mocked(api.generatedThumbnail).mock.calls).toEqual([
      ["c1", video.id, "after-text", image], ["c1", short.id, "after-text", image],
    ]);
    expect(api.setPrivacy).not.toHaveBeenCalled();
  });
  it("supports a single video's description without changing title or requesting a cover", async () => {
    const api = commands(); mount(api);
    fireEvent.click(await screen.findByRole("button", { name: "AI 优化 原剧名" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "标题" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "封面" }));
    await generate();
    fireEvent.click(screen.getByRole("button", { name: "一键同步到 YouTube" }));
    await screen.findByText("所选内容已全部同步");
    expect(api.generateAi).toHaveBeenCalledTimes(1);
    expect(api.update).toHaveBeenCalledWith(expect.objectContaining({ title: video.title, description: "新剧情与链接", privacyStatus: "private" }));
    expect(api.generatedThumbnail).not.toHaveBeenCalled();
  });
  it("retains generated text on an image quota failure and only generates missing stages on retry", async () => {
    const api = commands(); const normal = api.generateAi;
    vi.mocked(api.generateAi).mockImplementationOnce(async () => ({ video, title: "已生成标题", description: "已生成说明", image: null, model: "test" }))
      .mockRejectedValueOnce({ code: "AI_QUOTA_EXCEEDED", message: "图片额度不足" });
    mount(api); await openBatch();
    fireEvent.click(screen.getByRole("button", { name: "一键生成" }));
    await screen.findByText("图片额度不足");
    expect(normal).toHaveBeenCalledTimes(2);
    expect(screen.getByLabelText(`AI 标题 ${video.id}`)).toHaveProperty("value", "已生成标题");
    fireEvent.click(screen.getByRole("button", { name: "生成未完成项" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "生成未完成项" })).toHaveProperty("disabled", true));
    expect(vi.mocked(api.generateAi).mock.calls.filter(([, id, , kind]) => id === video.id && kind === "text")).toHaveLength(1);
    expect(api.update).not.toHaveBeenCalled();
  });
  it("does not repeat a confirmed cover upload when its readback fails", async () => {
    const api = commands([video]); mount(api);
    fireEvent.click(await screen.findByRole("button", { name: "AI 优化 原剧名" }));
    await generate();
    vi.mocked(api.detail).mockRejectedValueOnce({ message: "network" });
    fireEvent.click(screen.getByRole("button", { name: "一键同步到 YouTube" }));
    await screen.findByText(/封面已上传，读取最新资料失败/);
    fireEvent.click(screen.getByRole("button", { name: "一键同步到 YouTube" }));
    await screen.findByText("所选内容已全部同步");
    expect(api.update).toHaveBeenCalledTimes(1); expect(api.generatedThumbnail).toHaveBeenCalledTimes(1);
    expect(api.detail).toHaveBeenCalledTimes(2);
  });
  it("stops after a pending request when the channel changes and blocks duplicate generation", async () => {
    const api = commands(); let resolve!: (draft: VideoAiDraft) => void;
    vi.mocked(api.generateAi).mockImplementationOnce(() => new Promise(done => { resolve = done; }));
    const view = mount(api); await openBatch();
    const button = screen.getByRole("button", { name: "一键生成" }); fireEvent.click(button); fireEvent.click(button);
    expect(api.generateAi).toHaveBeenCalledTimes(1);
    view.rerender(<YouTubeManagement channelId="c2" channelTitle="其他频道" commands={api} />);
    await act(async () => resolve({ video, title: "过期结果", description: "不应使用", image: null, model: "test" }));
    expect(screen.queryByRole("dialog")).toBeNull(); expect(api.generateAi).toHaveBeenCalledTimes(1);
    expect(api.update).not.toHaveBeenCalled();
  });
  it("stops remaining stages on request and preserves the returned text", async () => {
    const api = commands(); let resolve!: (draft: VideoAiDraft) => void;
    vi.mocked(api.generateAi).mockImplementationOnce(() => new Promise(done => { resolve = done; }));
    mount(api); await openBatch(); fireEvent.click(screen.getByRole("button", { name: "一键生成" }));
    fireEvent.click(screen.getByRole("button", { name: "停止后续" }));
    await act(async () => resolve({ video, title: "保留标题", description: "保留说明", image: null, model: "test" }));
    expect(screen.getByLabelText(`AI 标题 ${video.id}`)).toHaveProperty("value", "保留标题");
    expect(api.generateAi).toHaveBeenCalledTimes(1);
  });
  it("rejects stale previews and validates edited description bytes before saving", async () => {
    const api = commands([video]); mount(api);
    fireEvent.click(await screen.findByRole("button", { name: "AI 优化 原剧名" }));
    vi.mocked(api.generateAi).mockResolvedValueOnce({ video: { ...video, etag: "changed" }, title: "stale", description: "stale", image: null, model: "test" });
    fireEvent.click(screen.getByRole("button", { name: "一键生成" }));
    await screen.findByText(/视频资料已变化/);
    expect(screen.getByLabelText(`AI 标题 ${video.id}`)).toHaveProperty("value", video.title);
    expect(api.update).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "生成未完成项" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "生成未完成项" })).toHaveProperty("disabled", true));
    fireEvent.change(screen.getByLabelText(`AI 说明 ${video.id}`), { target: { value: "中".repeat(1667) } });
    expect(screen.getByRole("button", { name: "一键同步到 YouTube" })).toHaveProperty("disabled", true);
    expect(api.update).not.toHaveBeenCalled();
  });
  it("retains completed text updates when cover upload fails and stops on channel-wide auth errors", async () => {
    const api = commands(); mount(api); await openBatch(); await generate();
    vi.mocked(api.generatedThumbnail).mockRejectedValueOnce({ code: "AUTH_REQUIRED", message: "请重新授权" });
    fireEvent.click(screen.getByRole("button", { name: "一键同步到 YouTube" }));
    await screen.findByText("请重新授权");
    expect(api.update).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "一键同步到 YouTube" }));
    await screen.findByText("所选内容已全部同步");
    expect(vi.mocked(api.update).mock.calls.map(([request]) => request.videoId)).toEqual([video.id, short.id]);
    expect(vi.mocked(api.generatedThumbnail).mock.calls.map(([, id]) => id)).toEqual([video.id, video.id, short.id]);
  });
  it("asks before discarding generated drafts and traps keyboard focus", async () => {
    mount(commands([video])); fireEvent.click(await screen.findByRole("button", { name: "AI 优化 原剧名" })); await generate();
    const dialog = screen.getByRole("dialog"); dialog.focus(); fireEvent.keyDown(dialog, { key: "Tab" });
    expect(document.activeElement).toBe(within(dialog).getByLabelText(`AI 标题 ${video.id}`));
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(screen.getByText("关闭将丢弃尚未同步的预览。")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "放弃预览并关闭" })); expect(screen.queryByRole("dialog")).toBeNull();
  });
});
