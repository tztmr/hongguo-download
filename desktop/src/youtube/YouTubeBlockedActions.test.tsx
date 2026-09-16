import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { YouTubeManagement } from "./YouTubeManagement";
import type { ManagedVideo, ManagementCommands } from "./managementCommands";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
afterEach(cleanup);
const normal: ManagedVideo = {
  id: "normal00001", title: "正常视频", description: "简介", etag: "rev1", privacyStatus: "public", thumbnailUrl: "", publishedAt: "",
  videoFormat: "standard", durationSeconds: 600, restriction: { kind: "noneReported", reason: "API 未返回封锁信息", allowedRegions: null, blockedRegions: [] },
};
const global: ManagedVideo = { ...normal, id: "global00001", title: "全球封锁正片", restriction: { kind: "global", reason: "全球封锁", allowedRegions: [], blockedRegions: [] } };
const regional: ManagedVideo = { ...normal, id: "region00001", title: "地区限制 Shorts", privacyStatus: "unlisted", videoFormat: "shorts", restriction: { kind: "region", reason: "地区限制", allowedRegions: null, blockedRegions: ["US"] } };
const copyright: ManagedVideo = { ...normal, id: "claims00001", title: "版权拒绝视频", restriction: { ...normal.restriction, kind: "copyright", reason: "版权原因被拒绝" } };
const alreadyPrivate: ManagedVideo = { ...global, id: "private0001", title: "已是私人的封锁视频", privacyStatus: "private" };
const unavailable: ManagedVideo = { ...normal, id: "failure0001", title: "处理失败", restriction: { ...normal.restriction, kind: "unavailable", reason: "处理失败" } };
const rows = [normal, global, regional, copyright, alreadyPrivate, unavailable];
function commands(items = rows): ManagementCommands {
  return {
    list: vi.fn().mockResolvedValue({ items, nextPageToken: null }),
    detail: vi.fn(), update: vi.fn(), thumbnail: vi.fn(), playlists: vi.fn(), membership: vi.fn(), createPlaylist: vi.fn(),
    lookup: vi.fn().mockResolvedValue({ items: [], failures: [] }), deleteVideo: vi.fn().mockResolvedValue(undefined),
    makeBlockedVideoPrivate: vi.fn().mockImplementation(async (_channel, id) => ({ ...rows.find((row) => row.id === id)!, privacyStatus: "private", etag: "private-rev" })),
  };
}
async function ready() {
  await waitFor(() => expect(screen.getByRole("button", { name: "一键删除封锁视频" })).toHaveProperty("disabled", false));
}
function mount(api: ManagementCommands) { return render(<YouTubeManagement channelId="c1" channelTitle="测试频道" commands={api} />); }

describe("channel-wide blocked video actions", () => {
  it("rescans every page, deduplicates, and deletes blocked videos and Shorts after reviewing the full channel", async () => {
    const api = commands();
    const firstPage = Array.from({ length: 50 }, (_, i) => ({ ...normal, id: `normal${i}`, title: `普通视频 ${i}` }));
    vi.mocked(api.list).mockResolvedValueOnce({ items: [normal], nextPageToken: null })
      .mockResolvedValueOnce({ items: [...firstPage, global], nextPageToken: "p2" })
      .mockResolvedValueOnce({ items: [global, regional], nextPageToken: "p3" })
      .mockResolvedValueOnce({ items: [copyright, alreadyPrivate, unavailable], nextPageToken: null });
    mount(api); await ready();
    // Toolbar filters and selection must not narrow the one-click channel scan.
    fireEvent.change(screen.getByLabelText("搜索频道视频"), { target: { value: "不会匹配" } });
    fireEvent.change(screen.getByLabelText("筛选视频类型"), { target: { value: "standard" } });
    fireEvent.change(screen.getByLabelText("筛选视频可见性"), { target: { value: "public" } });
    fireEvent.click(screen.getByRole("button", { name: "一键删除封锁视频" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(api.list).toHaveBeenNthCalledWith(2, "c1", undefined);
    expect(api.list).toHaveBeenNthCalledWith(3, "c1", "p2");
    expect(api.list).toHaveBeenNthCalledWith(4, "c1", "p3");
    expect(within(dialog).getAllByRole("listitem")).toHaveLength(4);
    expect(within(dialog).queryByText("处理失败")).toBeNull();
    expect(api.deleteVideo).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "确认永久删除 4 个" }));
    await screen.findByText("所选视频已全部删除");
    expect(vi.mocked(api.deleteVideo).mock.calls).toEqual([global, regional, copyright, alreadyPrivate].map((row) => ["c1", row.id]));
    expect(api.makeBlockedVideoPrivate).not.toHaveBeenCalled();
  });

  it("privatizes all eligible types, skips existing private videos, preserves rows, and blocks repeated submission", async () => {
    const api = commands();
    let resolve!: (value: ManagedVideo) => void;
    vi.mocked(api.makeBlockedVideoPrivate).mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    mount(api); await ready();
    fireEvent.click(screen.getByRole("button", { name: "一键设为私人" }));
    const dialog = await screen.findByRole("dialog", { name: "封锁视频批量设为私人" });
    expect(within(dialog).getByText("已跳过 1 个私人封锁视频。")).toBeTruthy();
    expect(within(dialog).getAllByRole("listitem")).toHaveLength(3);
    expect(api.makeBlockedVideoPrivate).not.toHaveBeenCalled();
    const confirm = within(dialog).getByRole("button", { name: "确认设为私人 3 个" });
    fireEvent.click(confirm); fireEvent.click(confirm);
    expect(api.makeBlockedVideoPrivate).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "关闭" })).toHaveProperty("disabled", true);
    expect(screen.getByRole("button", { name: "一键删除封锁视频", hidden: true })).toHaveProperty("disabled", true);
    await act(async () => { resolve({ ...global, privacyStatus: "private", etag: "fresh" }); });
    await screen.findByText("所选视频已全部设为私人");
    expect(vi.mocked(api.makeBlockedVideoPrivate).mock.calls).toEqual([global, regional, copyright].map((row) => ["c1", row.id]));
    fireEvent.click(screen.getByRole("button", { name: "关闭" }));
    fireEvent.change(screen.getByLabelText("筛选视频可见性"), { target: { value: "private" } });
    expect(screen.getAllByRole("checkbox", { name: /^选择 / })).toHaveLength(4);
    expect(api.update).not.toHaveBeenCalled(); expect(api.deleteVideo).not.toHaveBeenCalled();
  });

  it("retains failed and unattempted private changes and retries only those IDs", async () => {
    const api = commands([global, regional, copyright]);
    vi.mocked(api.makeBlockedVideoPrivate).mockResolvedValueOnce({ ...global, privacyStatus: "private" })
      .mockRejectedValueOnce({ code: "YOUTUBE_QUOTA_EXCEEDED", message: "配额已用完" });
    mount(api); await ready();
    fireEvent.click(screen.getByRole("button", { name: "一键设为私人" }));
    fireEvent.click(await screen.findByRole("button", { name: "确认设为私人 3 个" }));
    const retry = await screen.findByRole("button", { name: "重试剩余 2 个" });
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).queryByText(global.title)).toBeNull();
    expect(within(dialog).getByText("配额已用完")).toBeTruthy();
    expect(within(dialog).getByText("尚未执行：配额已用完")).toBeTruthy();
    expect(api.makeBlockedVideoPrivate).toHaveBeenCalledTimes(2);
    fireEvent.click(retry);
    await screen.findByText("所选视频已全部设为私人");
    expect(vi.mocked(api.makeBlockedVideoPrivate).mock.calls).toEqual([global, regional, regional, copyright].map((row) => ["c1", row.id]));
  });

  it("continues after a video-specific failure and never reports unconfirmed privacy as success", async () => {
    const api = commands([global, regional]);
    vi.mocked(api.makeBlockedVideoPrivate).mockRejectedValueOnce({ code: "YOUTUBE_VIDEO_NOT_BLOCKED", message: "封锁状态已改变" })
      .mockResolvedValueOnce(regional);
    mount(api); await ready();
    fireEvent.click(screen.getByRole("button", { name: "一键设为私人" }));
    fireEvent.click(await screen.findByRole("button", { name: "确认设为私人 2 个" }));
    await screen.findByRole("button", { name: "重试剩余 2 个" });
    expect(api.makeBlockedVideoPrivate).toHaveBeenCalledTimes(2);
    expect(screen.getByText("封锁状态已改变")).toBeTruthy();
    expect(screen.getByText("YouTube 尚未确认设为私人，请刷新核对后重试")).toBeTruthy();
    expect(screen.queryByText("所选视频已全部设为私人")).toBeNull();
  });

  it.each(["network", "repeat"])("does not open a bulk action on an incomplete %s scan and allows a fresh retry", async (failure) => {
    const api = commands([global]);
    vi.mocked(api.list).mockResolvedValueOnce({ items: [normal], nextPageToken: null })
      .mockResolvedValueOnce({ items: [global], nextPageToken: "p2" });
    if (failure === "network") vi.mocked(api.list).mockRejectedValueOnce({ message: "连接中断" });
    else vi.mocked(api.list).mockResolvedValueOnce({ items: [regional], nextPageToken: "p2" });
    mount(api); await ready();
    fireEvent.click(screen.getByRole("button", { name: "一键删除封锁视频" }));
    expect((await screen.findByRole("alert")).textContent).toContain("未执行批量操作");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(api.deleteVideo).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "一键删除封锁视频" }));
    await screen.findByRole("button", { name: "确认永久删除 1 个" });
    expect(api.list).toHaveBeenLastCalledWith("c1", undefined);
  });

  it("cancels scan preparation without mutations and blocks simultaneous scan buttons", async () => {
    const api = commands();
    let resolve!: (page: Awaited<ReturnType<ManagementCommands["list"]>>) => void;
    vi.mocked(api.list).mockResolvedValueOnce({ items: [normal], nextPageToken: null })
      .mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    mount(api); await ready();
    fireEvent.click(screen.getByRole("button", { name: "一键设为私人" }));
    fireEvent.click(screen.getByRole("button", { name: "一键删除封锁视频" }));
    expect(api.list).toHaveBeenCalledTimes(2);
    fireEvent.click(screen.getByRole("button", { name: "取消扫描" }));
    await act(async () => { resolve({ items: [global], nextPageToken: "more" }); });
    await screen.findByText("已取消封锁视频扫描，未执行批量操作。");
    expect(api.list).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(api.makeBlockedVideoPrivate).not.toHaveBeenCalled();
  });

  it.each(["scan", "update"])("stops the old channel's %s when switching accounts", async (stage) => {
    const api = commands([global, regional]);
    let finish!: () => void;
    if (stage === "scan") vi.mocked(api.list).mockResolvedValueOnce({ items: [normal], nextPageToken: null })
      .mockImplementationOnce(() => new Promise((done) => { finish = () => done({ items: [global], nextPageToken: "p2" }); }));
    else vi.mocked(api.makeBlockedVideoPrivate).mockImplementationOnce(() => new Promise((done) => { finish = () => done({ ...global, privacyStatus: "private" }); }));
    const view = mount(api); await ready();
    fireEvent.click(screen.getByRole("button", { name: "一键设为私人" }));
    if (stage === "update") fireEvent.click(await screen.findByRole("button", { name: "确认设为私人 2 个" }));
    vi.mocked(api.list).mockResolvedValue({ items: [], nextPageToken: null });
    view.rerender(<YouTubeManagement channelId="new" channelTitle="新频道" commands={api} />);
    await act(async () => { finish(); });
    await screen.findByText("频道暂无可管理的视频");
    expect(api.list).toHaveBeenCalledTimes(3);
    expect(api.makeBlockedVideoPrivate).toHaveBeenCalledTimes(stage === "update" ? 1 : 0);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("explains empty results and skips all private targets without issuing writes", async () => {
    const api = commands([normal, unavailable]);
    mount(api); await ready();
    fireEvent.click(screen.getByRole("button", { name: "一键删除封锁视频" }));
    await screen.findByText("全频道扫描完成，未发现 API 标记的封锁／地区限制／版权拒绝视频。");
    vi.mocked(api.list).mockResolvedValue({ items: [alreadyPrivate], nextPageToken: null });
    fireEvent.click(screen.getByRole("button", { name: "一键设为私人" }));
    await screen.findByText("全频道扫描完成，1 个封锁视频均已是私人，无需修改。");
    expect(api.deleteVideo).not.toHaveBeenCalled(); expect(api.makeBlockedVideoPrivate).not.toHaveBeenCalled();
  });
});
