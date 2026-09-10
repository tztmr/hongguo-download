import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { open } from "@tauri-apps/plugin-dialog";
import { YouTubeManagement } from "./YouTubeManagement";
import type { ManagementCommands, ManagedVideo } from "./managementCommands";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
afterEach(cleanup);
const video: ManagedVideo = { id: "video1", etag: "v1", title: "原来的标题", description: "原简介", privacyStatus: "private", thumbnailUrl: "", publishedAt: "2026-09-01T00:00:00Z" };
function commands(): ManagementCommands {
  return { detail: vi.fn().mockResolvedValue(video), list: vi.fn().mockResolvedValue({ items: [video], nextPageToken: "page2" }),
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

});
