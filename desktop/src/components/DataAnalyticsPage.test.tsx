import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { previewYouTubeModel } from "../preview";
import type { AnalyticsCommands, AnalyticsMetrics } from "../youtube/analyticsCommands";
import { DataAnalyticsPage } from "./DataAnalyticsPage";

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

const metrics: AnalyticsMetrics = { views: 3210, engagedViews: 2800, estimatedMinutesWatched: 987.5, averageViewDuration: 42,
  averageViewPercentage: 108.2, likes: 103, comments: null, shares: 0, subscribersGained: 5, subscribersLost: 12 };
function commands(): AnalyticsCommands {
  return {
    snapshot: vi.fn().mockResolvedValue({ channelId: "UC_PREVIEW", viewCount: "123456", subscriberCount: "2340", hiddenSubscriberCount: false, videoCount: "58", fetchedAt: "2026-09-10T10:00:00Z" }),
    report: vi.fn().mockResolvedValue({
      channelId: "UC_PREVIEW", videoId: null, startDate: "2026-08-15", endDate: "2026-09-10", returnedEndDate: "2026-09-09",
      ...metrics, fetchedAt: "2026-09-10T10:00:00Z", comparison: { ...metrics, views: 0, startDate: "2026-07-20", endDate: "2026-08-14" }, warnings: [],
      timezone: "America/Los_Angeles", rows: [{ date: "2026-09-09", ...metrics }],
    }),
    breakdown: vi.fn(async (channelId, startDate, endDate, kind, videoId) => ({ channelId, videoId: videoId ?? null, startDate, endDate, kind, rows: [], warnings: [], truncated: false, fetchedAt: "2026-09-10T10:00:00Z" })),
  };
}

describe("DataAnalyticsPage", () => {
  it("loads cumulative and historical channel statistics with the selected range", async () => {
    const api = commands();
    render(<DataAnalyticsPage youtube={previewYouTubeModel} commands={api} />);

    expect(await screen.findByText("123,456")).toBeTruthy();
    expect(screen.getByText("3,210", { selector: "strong" })).toBeTruthy();
    expect(screen.getAllByText("16.5 小时")).toHaveLength(2);
    expect(screen.getByText("2026/09/09")).toBeTruthy();
    expect(screen.getByText(/最近日期可能仍在处理/)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "7 天" }));
    await waitFor(() => expect(api.report).toHaveBeenLastCalledWith("UC_PREVIEW", expect.any(String), expect.any(String)));
    expect(api.report).toHaveBeenCalledTimes(2);
  });

  it("shows a reauthorization action when Analytics permission is missing", async () => {
    const api = commands();
    vi.mocked(api.report).mockRejectedValue({ message: "需要统计权限，请在设置中重新授权 YouTube 频道" });
    const authorize = vi.spyOn(previewYouTubeModel, "authorize").mockResolvedValue(undefined);
    render(<DataAnalyticsPage youtube={previewYouTubeModel} commands={api} />);

    expect((await screen.findByRole("alert")).textContent).toContain("需要统计权限");
    fireEvent.click(screen.getByRole("button", { name: "重新授权统计权限" }));
    await waitFor(() => expect(authorize).toHaveBeenCalled());
  });
});

it("shows cumulative data while the historical request is still pending", async () => {
  const api = commands();
  vi.mocked(api.report).mockReturnValue(new Promise(() => {}));
  render(<DataAnalyticsPage youtube={previewYouTubeModel} commands={api} />);
  expect(await screen.findByText("123,456")).toBeTruthy();
  expect(screen.getByText("正在读取 YouTube 统计…")).toBeTruthy();
});

it("keeps previous data visible while refreshing and on refresh failure", async () => {
  const api = commands();
  render(<DataAnalyticsPage youtube={previewYouTubeModel} commands={api} />);
  await screen.findByText("123,456");
  await waitFor(() => expect((screen.getByRole("button", { name: "刷新数据" }) as HTMLButtonElement).disabled).toBe(false));
  vi.mocked(api.report).mockRejectedValue({ code: "YOUTUBE_QUOTA_EXCEEDED", message: "配额已用完" });
  fireEvent.click(screen.getByRole("button", { name: "刷新数据" }));
  expect(screen.getByText("3,210", { selector: "strong" })).toBeTruthy();
  expect((await screen.findByRole("alert")).textContent).toContain("配额已用完");
  expect(screen.getByText("3,210", { selector: "strong" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "重新授权统计权限" })).toBeNull();
});

it("only queries a custom date range after submission", async () => {
  const api = commands();
  render(<DataAnalyticsPage youtube={previewYouTubeModel} commands={api} />);
  await screen.findByText("123,456");
  fireEvent.click(screen.getByRole("button", { name: "自定义" }));
  fireEvent.change(screen.getByLabelText("开始日期"), { target: { value: "2026-01-01" } });
  expect(api.report).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole("button", { name: "查询" }));
  await waitFor(() => expect(api.report).toHaveBeenCalledTimes(2));
  expect(api.report).toHaveBeenLastCalledWith("UC_PREVIEW", "2026-01-01", expect.any(String));
});

it("routes disabled API errors to configuration without asking to reauthorize", async () => {
  const api = commands();
  vi.mocked(api.report).mockRejectedValue({ code: "YOUTUBE_ANALYTICS_API_NOT_ENABLED", message: "API 未启用" });
  render(<DataAnalyticsPage youtube={previewYouTubeModel} commands={api} />);
  expect(await screen.findByRole("link", { name: "前往启用 Analytics API" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "重新授权统计权限" })).toBeNull();
});

it("disables repeated authorization requests until the browser flow finishes", async () => {
  const api = commands();
  vi.mocked(api.report).mockRejectedValue({ code: "YOUTUBE_ANALYTICS_AUTH_REQUIRED", message: "需要统计权限" });
  let resolve!: () => void;
  const authorize = vi.fn(() => new Promise<void>(done => { resolve = done; }));
  render(<DataAnalyticsPage youtube={{ ...previewYouTubeModel, authorize }} commands={api} />);
  fireEvent.click(await screen.findByRole("button", { name: "重新授权统计权限" }));
  expect(screen.getByRole("status").textContent).toContain("浏览器");
  expect(authorize).toHaveBeenCalledTimes(1);
  await act(async () => resolve());
});

it("does not show the previous channel's statistics after an account switch", async () => {
  const api = commands();
  const view = render(<DataAnalyticsPage youtube={previewYouTubeModel} commands={api} />);
  await screen.findByText("123,456");
  vi.mocked(api.snapshot).mockReturnValue(new Promise(() => {}));
  vi.mocked(api.report).mockReturnValue(new Promise(() => {}));
  view.rerender(<DataAnalyticsPage youtube={{ ...previewYouTubeModel, activeChannelId: "other" }} commands={api} />);
  expect(screen.queryByText("123,456")).toBeNull();
});

it("distinguishes missing metrics, actual zeroes, negative subscribers and average percentages above 100", async () => {
  const api = commands();
  render(<DataAnalyticsPage youtube={previewYouTubeModel} commands={api} />);
  await screen.findByText("123,456");
  const engagement = screen.getByLabelText("互动与订阅明细");
  expect(within(engagement).getByText("评论").nextElementSibling?.textContent).toBe("—");
  expect(within(engagement).getByText("分享").nextElementSibling?.textContent).toBe("0");
  expect(screen.getByText("净增订阅", { selector: "span" }).nextElementSibling?.textContent).toBe("-7");
  expect(screen.getAllByText("108.2%").length).toBeGreaterThan(0);
  expect(screen.getByText("上期为 0")).toBeTruthy();
  expect(screen.getByText(/2026\/07\/20 至 2026\/08\/14/)).toBeTruthy();
  expect(screen.getByText("平均播放比例，非完播率")).toBeTruthy();
});

it("loads only the selected detail report and enters and leaves a single video scope", async () => {
  const api = commands();
  vi.mocked(api.breakdown).mockImplementation(async (channelId, startDate, endDate, kind, videoId) => ({
    channelId, startDate, endDate, kind, videoId: videoId ?? null, rows: kind === "videos" ? [
      { ...metrics, key: "demoVideo01", title: "演示 Shorts", thumbnailUrl: null, contentType: "SHORTS" },
    ] : [], warnings: [], truncated: false, fetchedAt: "2026-09-10T10:00:00Z",
  }));
  render(<DataAnalyticsPage youtube={previewYouTubeModel} commands={api} />);
  fireEvent.click(await screen.findByRole("button", { name: "查看 演示 Shorts 的趋势" }));
  await waitFor(() => expect(api.report).toHaveBeenLastCalledWith("UC_PREVIEW", expect.any(String), expect.any(String), "demoVideo01"));
  await waitFor(() => expect(api.breakdown).toHaveBeenLastCalledWith("UC_PREVIEW", "2026-08-15", "2026-09-09", "traffic", "demoVideo01"));
  expect(screen.getByRole("heading", { name: "单视频表现" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "热门视频" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "返回频道总览" }));
  await waitFor(() => expect(api.report).toHaveBeenLastCalledWith("UC_PREVIEW", expect.any(String), expect.any(String)));
  expect(await screen.findByRole("button", { name: "热门视频" })).toBeTruthy();
});

it("ignores late reports when dates change and removes the previous range immediately", async () => {
  const api = commands();
  const initial = await api.report("UC_PREVIEW", "2026-08-15", "2026-09-10");
  vi.mocked(api.report).mockClear();
  let resolveOld!: (value: typeof initial) => void;
  vi.mocked(api.report).mockReturnValueOnce(new Promise(resolve => { resolveOld = resolve; }));
  render(<DataAnalyticsPage youtube={previewYouTubeModel} commands={api} />);
  await screen.findByText("123,456");
  fireEvent.click(screen.getByRole("button", { name: "7 天" }));
  expect(await screen.findByText("3,210", { selector: "strong" })).toBeTruthy();
  await act(async () => resolveOld({ ...initial, views: 99999 }));
  expect(screen.queryByText("99,999")).toBeNull();
  vi.mocked(api.report).mockReturnValue(new Promise(() => {}));
  await act(async () => fireEvent.click(screen.getByRole("button", { name: "90 天" })));
  expect(screen.queryByText("3,210")).toBeNull();
  expect(screen.queryByRole("region", { name: "细分报表" })).toBeNull();
});

it("rejects custom ranges wider than 367 days before querying", async () => {
  const api = commands();
  render(<DataAnalyticsPage youtube={previewYouTubeModel} commands={api} />);
  await screen.findByText("123,456");
  fireEvent.click(screen.getByRole("button", { name: "自定义" }));
  fireEvent.change(screen.getByLabelText("开始日期"), { target: { value: "2020-01-01" } });
  expect(screen.getByRole("alert").textContent).toContain("367 天");
  expect((screen.getByRole("button", { name: "查询" }) as HTMLButtonElement).disabled).toBe(true);
  expect(api.report).toHaveBeenCalledTimes(1);
});
