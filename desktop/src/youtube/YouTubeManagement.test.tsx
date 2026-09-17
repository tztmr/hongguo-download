import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { open } from "@tauri-apps/plugin-dialog";
import { YouTubeManagement } from "./YouTubeManagement";
import type { ManagementCommands, ManagedVideo } from "./managementCommands";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
afterEach(cleanup);
const video: ManagedVideo = { id: "video1", etag: "v1", title: "原来的标题", description: "原简介", privacyStatus: "private", thumbnailUrl: "", publishedAt: "2026-09-01T00:00:00Z", videoFormat: "standard", durationSeconds: 900,
  restriction: { kind: "noneReported", reason: "API 未返回封锁信息", allowedRegions: null, blockedRegions: [] } };
function commands(): ManagementCommands {
  return { detail: vi.fn().mockResolvedValue(video), list: vi.fn().mockResolvedValue({ items: [video], nextPageToken: "page2" }),
    lookup: vi.fn().mockResolvedValue({ items: [], failures: [] }), deleteVideo: vi.fn().mockResolvedValue(undefined),
    makeBlockedVideoPrivate: vi.fn().mockResolvedValue(video),
    setPrivacy: vi.fn(), generateAi: vi.fn(), generatedThumbnail: vi.fn(),
    update: vi.fn().mockImplementation(async (request) => ({ ...video, ...request, id: video.id, etag: "v2" })),
    thumbnail: vi.fn().mockResolvedValue(undefined), playlists: vi.fn().mockResolvedValue([{ id: "pl1", title: "短剧合集", privacyStatus: "public", itemIds: [] }]),
    membership: vi.fn().mockResolvedValue(undefined), createPlaylist: vi.fn().mockResolvedValue({ id: "pl2", title: "新清单", privacyStatus: "private", itemIds: [] }) };
}
describe("YouTube management", () => {
  it("requires a selected authorized channel", () => {
    const api = commands(); render(<YouTubeManagement channelId={null} channelTitle="" commands={api} />);
    expect(screen.getByText(/请先在设置中授权/)).toBeTruthy(); expect(api.list).not.toHaveBeenCalled();
  });
  it("loads channel videos and saves the selected video's metadata with its revision", async () => {
    const api = commands(); render(<YouTubeManagement channelId="channel1" channelTitle="测试频道" commands={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑 原来的标题" }));
    fireEvent.change(screen.getByLabelText("视频标题"), { target: { value: "新标题" } });
    fireEvent.change(screen.getByLabelText("视频可见性"), { target: { value: "public" } });
    fireEvent.click(screen.getByRole("button", { name: "保存资料到 YouTube" }));
    await waitFor(() => expect(api.update).toHaveBeenCalledWith({ channelId: "channel1", videoId: "video1", etag: "v1", title: "新标题", description: "原简介", privacyStatus: "public" }));
    expect(await screen.findByText("资料已同步到 YouTube")).toBeTruthy();
  });
  it("keeps edits and shows a failed save instead of reporting success", async () => {
    const api = commands(); vi.mocked(api.update).mockRejectedValue({ message: "授权已过期" });
    render(<YouTubeManagement channelId="channel1" channelTitle="测试频道" commands={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑 原来的标题" }));
    fireEvent.change(screen.getByLabelText("视频标题"), { target: { value: "保留草稿" } });
    fireEvent.click(screen.getByRole("button", { name: "保存资料到 YouTube" }));
    expect(await screen.findByRole("alert")).toHaveProperty("textContent", "授权已过期");
    expect((screen.getByLabelText("视频标题") as HTMLInputElement).value).toBe("保留草稿");
  });
  it("paginates without treating the first page as the whole channel", async () => {
    const api = commands(); render(<YouTubeManagement channelId="channel1" channelTitle="测试频道" commands={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "加载更多视频" }));
    await waitFor(() => expect(api.list).toHaveBeenLastCalledWith("channel1", "page2"));
    expect(screen.getAllByRole("button", { name: "编辑 原来的标题" })).toHaveLength(1);
  });
  it("does not let a response from the previous channel populate the current channel", async () => {
    const api = commands(); let resolve!: (value: Awaited<ReturnType<ManagementCommands["list"]>>) => void;
    vi.mocked(api.list).mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    vi.mocked(api.list).mockResolvedValue({ items: [], nextPageToken: null });
    const view = render(<YouTubeManagement channelId="old" channelTitle="旧频道" commands={api} />);
    view.rerender(<YouTubeManagement channelId="new" channelTitle="新频道" commands={api} />);
    await screen.findByText("频道暂无可管理的视频"); resolve({ items: [video], nextPageToken: null });
    await waitFor(() => expect(screen.queryByRole("button", { name: "编辑 原来的标题" })).toBeNull());
  });
  it("only changes the playlist for the selected video and refreshes membership", async () => {
    const api = commands(); render(<YouTubeManagement channelId="channel1" channelTitle="测试频道" commands={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑 原来的标题" }));
    fireEvent.click(screen.getByRole("button", { name: "读取播放列表" }));
    fireEvent.click(await screen.findByRole("button", { name: "加入 短剧合集" }));
    await waitFor(() => expect(api.membership).toHaveBeenCalledWith("channel1", "video1", "pl1", true));
    expect(await screen.findByText("播放列表已同步到 YouTube")).toBeTruthy();
    expect(api.update).not.toHaveBeenCalled();
  });
  it("requires an explicit discard to close unsaved edits", async () => {
    render(<YouTubeManagement channelId="channel1" channelTitle="测试频道" commands={commands()} />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑 原来的标题" }));
    fireEvent.change(screen.getByLabelText("视频标题"), { target: { value: "未保存" } });
    fireEvent.click(screen.getByRole("button", { name: "完成" }));
    expect(screen.getByRole("dialog")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "放弃修改并关闭" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });
  it("rejects a description over YouTube's UTF-8 byte limit before sending", async () => {
    const api = commands(); render(<YouTubeManagement channelId="channel1" channelTitle="测试频道" commands={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑 原来的标题" }));
    fireEvent.change(screen.getByLabelText("视频简介"), { target: { value: "中".repeat(1667) } });
    expect((screen.getByRole("button", { name: "保存资料到 YouTube" }) as HTMLButtonElement).disabled).toBe(true);
    expect(api.update).not.toHaveBeenCalled();
  });

  it("refreshes the revision after a thumbnail upload while preserving unsaved text", async () => {
    const api = commands(); vi.mocked(open).mockResolvedValue("/synthetic/cover.jpg");
    vi.mocked(api.detail).mockResolvedValue({ ...video, etag: "after-cover" });
    render(<YouTubeManagement channelId="channel1" channelTitle="测试频道" commands={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑 原来的标题" }));
    fireEvent.change(screen.getByLabelText("视频标题"), { target: { value: "尚未保存的标题" } });
    fireEvent.click(screen.getByRole("button", { name: "选择本地封面" }));
    await screen.findByText("/synthetic/cover.jpg");
    fireEvent.click(screen.getByRole("button", { name: "上传新封面" }));
    await waitFor(() => expect(api.detail).toHaveBeenCalledWith("channel1", "video1"));
    await waitFor(() => expect((screen.getByRole("button", { name: "保存资料到 YouTube" }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "保存资料到 YouTube" }));
    await waitFor(() => expect(api.update).toHaveBeenCalledWith(expect.objectContaining({ title: "尚未保存的标题", etag: "after-cover" })));
  });

  it("loads every page including Shorts beyond the first 50 videos and deduplicates IDs", async () => {
    const api = commands();
    const firstPage = Array.from({ length: 50 }, (_, i) => ({ ...video, id: `long${i}`, title: `正片 ${i}` }));
    const short = { ...video, id: "short000001", title: "第二页首集 Shorts", videoFormat: "shorts" as const };
    vi.mocked(api.list).mockResolvedValueOnce({ items: firstPage, nextPageToken: "p2" })
      .mockResolvedValueOnce({ items: [firstPage[0], short], nextPageToken: "p3" })
      .mockResolvedValueOnce({ items: [{ ...video, id: "tail", title: "最后一页" }], nextPageToken: null });
    render(<YouTubeManagement channelId="channel1" channelTitle="测试频道" commands={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "加载全部视频与 Shorts" }));
    await screen.findByText("已加载全部频道视频与 Shorts");
    expect(api.list).toHaveBeenNthCalledWith(2, "channel1", "p2");
    expect(api.list).toHaveBeenNthCalledWith(3, "channel1", "p3");
    expect(screen.getAllByRole("checkbox", { name: /^选择 / })).toHaveLength(52);
    fireEvent.change(screen.getByLabelText("筛选视频类型"), { target: { value: "shorts" } });
    expect(screen.getAllByRole("checkbox", { name: /^选择 / })).toHaveLength(1);
    expect(screen.getByRole("link", { name: "打开 YouTube 视频" }).getAttribute("href")).toContain("/shorts/short000001");
  });

  it("keeps loaded pages on failure and resumes from the failed token", async () => {
    const api = commands();
    vi.mocked(api.list).mockResolvedValueOnce({ items: [video], nextPageToken: "p2" })
      .mockRejectedValueOnce({ message: "连接中断" })
      .mockResolvedValueOnce({ items: [{ ...video, id: "short", title: "重试读取的 Shorts" }], nextPageToken: null });
    render(<YouTubeManagement channelId="channel1" channelTitle="测试频道" commands={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "加载全部视频与 Shorts" }));
    expect(await screen.findByRole("alert")).toHaveProperty("textContent", "连接中断");
    expect(screen.queryByText("已加载全部频道视频与 Shorts")).toBeNull();
    expect(screen.getByRole("checkbox", { name: "选择 原来的标题" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "加载全部视频与 Shorts" }));
    await screen.findByText("重试读取的 Shorts");
    expect(api.list).toHaveBeenLastCalledWith("channel1", "p2");
  });

  it("stops repeated pagination without claiming the channel is complete", async () => {
    const api = commands();
    render(<YouTubeManagement channelId="channel1" channelTitle="测试频道" commands={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "加载全部视频与 Shorts" }));
    expect(await screen.findByRole("alert")).toHaveProperty("textContent", expect.stringContaining("重复分页"));
    expect(api.list).toHaveBeenCalledTimes(2);
    expect(screen.queryByText("已加载全部频道视频与 Shorts")).toBeNull();
  });

  it("filters restrictions and only selects visible results, clearing selection when filters change", async () => {
    const api = commands();
    const blocked = { ...video, id: "blocked", title: "封锁 Shorts", videoFormat: "shorts" as const, restriction: { ...video.restriction, kind: "global" as const, reason: "全球封锁", allowedRegions: [] } };
    vi.mocked(api.list).mockResolvedValue({ items: [video, blocked], nextPageToken: null });
    render(<YouTubeManagement channelId="channel1" channelTitle="测试频道" commands={api} />);
    fireEvent.click(await screen.findByRole("checkbox", { name: "选择 原来的标题" }));
    fireEvent.change(screen.getByLabelText("筛选视频限制"), { target: { value: "blocked" } });
    expect(screen.getByRole("button", { name: "批量删除（0）" })).toHaveProperty("disabled", true);
    fireEvent.click(screen.getByRole("checkbox", { name: "全选当前筛选结果" }));
    fireEvent.click(screen.getByRole("button", { name: "批量删除（1）" }));
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).queryByText("原来的标题")).toBeNull();
    expect(api.deleteVideo).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(api.deleteVideo).not.toHaveBeenCalled();
  });

  it("deletes all five selected videos and Shorts only after confirmation and blocks double submission", async () => {
    const api = commands();
    const rows = Array.from({ length: 5 }, (_, i) => ({ ...video, id: `selected${i}`, title: `选中视频 ${i}`, videoFormat: i % 2 ? "shorts" as const : "standard" as const }));
    vi.mocked(api.list).mockResolvedValue({ items: rows, nextPageToken: null });
    let resolve!: () => void;
    vi.mocked(api.deleteVideo).mockImplementationOnce(() => new Promise<void>((done) => { resolve = done; }));
    render(<YouTubeManagement channelId="channel1" channelTitle="测试频道" commands={api} />);
    await screen.findByText("选中视频 0");
    for (const row of rows) fireEvent.click(screen.getByRole("checkbox", { name: `选择 ${row.title}` }));
    fireEvent.click(screen.getByRole("button", { name: "批量删除（5）" }));
    expect(api.deleteVideo).not.toHaveBeenCalled();
    const confirm = screen.getByRole("button", { name: "确认永久删除 5 个" });
    fireEvent.click(confirm); fireEvent.click(confirm);
    expect(api.deleteVideo).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "刷新频道视频", hidden: true })).toHaveProperty("disabled", true);
    await act(async () => { resolve(); });
    await screen.findByText("所选视频已全部删除");
    expect(api.deleteVideo).toHaveBeenCalledTimes(5);
    rows.forEach((row, i) => expect(api.deleteVideo).toHaveBeenNthCalledWith(i + 1, "channel1", row.id));
    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    expect(screen.queryAllByRole("checkbox", { name: /^选择 / })).toHaveLength(0);
  });

  it("keeps failed and unattempted videos, shows causes, and retries only the remainder", async () => {
    const api = commands();
    const rows = ["第一条", "第二条", "第三条"].map((title, i) => ({ ...video, id: `v${i}`, title }));
    vi.mocked(api.list).mockResolvedValue({ items: rows, nextPageToken: null });
    vi.mocked(api.deleteVideo).mockResolvedValueOnce(undefined).mockRejectedValueOnce({ code: "YOUTUBE_QUOTA_EXCEEDED", message: "配额已用完" });
    render(<YouTubeManagement channelId="channel1" channelTitle="测试频道" commands={api} />);
    await screen.findByText("第一条");
    fireEvent.click(screen.getByRole("checkbox", { name: "全选当前筛选结果" }));
    fireEvent.click(screen.getByRole("button", { name: "批量删除（3）" }));
    fireEvent.click(screen.getByRole("button", { name: "确认永久删除 3 个" }));
    await screen.findByRole("button", { name: "重试剩余 2 个" });
    expect(api.deleteVideo).toHaveBeenCalledTimes(2);
    expect(within(screen.getByRole("alertdialog")).queryByText("第一条")).toBeNull();
    expect(screen.getByText("配额已用完")).toBeTruthy();
    expect(screen.getByText("尚未执行：配额已用完")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "重试剩余 2 个" }));
    await screen.findByText("所选视频已全部删除");
    expect(api.deleteVideo).toHaveBeenCalledTimes(4);
    expect(api.deleteVideo).toHaveBeenNthCalledWith(3, "channel1", "v1");
    expect(api.deleteVideo).toHaveBeenNthCalledWith(4, "channel1", "v2");
  });

  it("queries notification links beyond loaded pages and keeps nonreported restrictions available for review", async () => {
    const api = commands();
    const short = { ...video, id: "short000001", title: "通知中封锁的 Shorts", videoFormat: "shorts" as const };
    vi.mocked(api.lookup).mockResolvedValue({ items: [short], failures: [{ videoId: "gone0000001", message: "视频不存在或无法读取" }] });
    render(<YouTubeManagement channelId="channel1" channelTitle="测试频道" commands={api} />);
    await screen.findByText("原来的标题");
    fireEvent.change(screen.getByLabelText("筛选视频限制"), { target: { value: "blocked" } });
    fireEvent.change(screen.getByLabelText("通知中的视频链接或 ID"), { target: { value: "https://youtube.com/shorts/short000001\nhttps://studio.youtube.com/video/gone0000001/copyright" } });
    fireEvent.click(screen.getByRole("button", { name: "查询通知视频", hidden: true }));
    await screen.findByText("通知中封锁的 Shorts");
    expect(api.lookup).toHaveBeenCalledWith("channel1", ["short000001", "gone0000001"]);
    expect(screen.queryByRole("checkbox", { name: "选择 原来的标题" })).toBeNull();
    expect(screen.getByRole("alert").textContent).toContain("gone0000001");
    expect(screen.getByLabelText("筛选视频限制")).toHaveProperty("value", "all");
    expect(api.deleteVideo).not.toHaveBeenCalled();
  });

  it("stops the remaining delete loop when the user changes channels", async () => {
    const api = commands();
    vi.mocked(api.list).mockResolvedValue({ items: [video, { ...video, id: "v2", title: "另一条" }], nextPageToken: null });
    let resolve!: () => void;
    vi.mocked(api.deleteVideo).mockImplementationOnce(() => new Promise<void>((done) => { resolve = done; }));
    const view = render(<YouTubeManagement channelId="old" channelTitle="旧频道" commands={api} />);
    await screen.findByText("原来的标题");
    fireEvent.click(screen.getByRole("checkbox", { name: "全选当前筛选结果" }));
    fireEvent.click(screen.getByRole("button", { name: "批量删除（2）" }));
    fireEvent.click(screen.getByRole("button", { name: "确认永久删除 2 个" }));
    vi.mocked(api.list).mockResolvedValue({ items: [], nextPageToken: null });
    view.rerender(<YouTubeManagement channelId="new" channelTitle="新频道" commands={api} />);
    await act(async () => { resolve(); });
    await screen.findByText("频道暂无可管理的视频");
    expect(api.deleteVideo).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

});
