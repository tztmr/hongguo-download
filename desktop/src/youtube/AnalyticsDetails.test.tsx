import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { AnalyticsDetails } from "./AnalyticsDetails";
import { AnalyticsTrend } from "./AnalyticsTrend";
import type { AnalyticsBreakdown, AnalyticsCommands, AnalyticsMetrics } from "./analyticsCommands";

afterEach(cleanup);
const metrics: AnalyticsMetrics = { views: 100, engagedViews: 80, estimatedMinutesWatched: 120,
  averageViewDuration: 90, averageViewPercentage: 42, likes: 4, comments: null, shares: 0, subscribersGained: 1, subscribersLost: 3 };
const base: AnalyticsBreakdown = { channelId: "c1", videoId: null, startDate: "2026-09-01", endDate: "2026-09-10",
  kind: "videos", truncated: true, warnings: [], fetchedAt: "2026-09-11T10:00:00Z", rows: [
    { ...metrics, key: "demoVideo00", title: "普通剧集", thumbnailUrl: null, contentType: "VIDEO_ON_DEMAND" },
    { ...metrics, key: "demoVideo01", title: "竖屏片段", thumbnailUrl: null, contentType: "SHORTS", views: 200, averageViewPercentage: 112 },
    { ...metrics, key: "demoVideo02", title: null, thumbnailUrl: null, contentType: null, views: null },
  ] };
function commands(): AnalyticsCommands {
  return { snapshot: vi.fn(), report: vi.fn(), breakdown: vi.fn(async (_channel, _start, _end, kind) => ({ ...base, kind })) };
}
function setup(api = commands()) {
  const props = { channelId: "c1", startDate: base.startDate, endDate: base.endDate, revision: base.fetchedAt, commands: api, onSelectVideo: vi.fn() };
  return { ...render(<AnalyticsDetails {...props} />), api, props };
}

it("filters Shorts and unknown types and searches only the returned top 200", async () => {
  const { api, props } = setup();
  await screen.findByText("普通剧集");
  expect(screen.getByText(/观看量前 200 条/).textContent).toContain("已返回的榜单");
  fireEvent.change(screen.getByLabelText("筛选热门视频类型"), { target: { value: "SHORTS" } });
  expect(screen.queryByText("普通剧集")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "查看 竖屏片段 的趋势" }));
  expect(props.onSelectVideo).toHaveBeenCalledWith({ id: "demoVideo01", title: "竖屏片段" });
  fireEvent.change(screen.getByLabelText("筛选热门视频类型"), { target: { value: "UNSPECIFIED" } });
  expect(screen.getByRole("button", { name: "查看 demoVideo02 的趋势" })).toBeTruthy();
  fireEvent.change(screen.getByLabelText("筛选热门视频类型"), { target: { value: "all" } });
  fireEvent.change(screen.getByLabelText("搜索热门视频"), { target: { value: " DEMOVIDEO01 " } });
  expect(screen.queryByText("普通剧集")).toBeNull();
  expect(screen.getByText("竖屏片段")).toBeTruthy();
  expect(api.breakdown).toHaveBeenCalledTimes(1);
});

it("isolates report failures, keeps successful data on refresh failure and reuses tabs", async () => {
  const { api } = setup();
  await screen.findByText("普通剧集");
  vi.mocked(api.breakdown).mockRejectedValue({ code: "YOUTUBE_ANALYTICS_FAILED", message: "来源报表暂不可用" });
  fireEvent.click(screen.getByRole("button", { name: "流量来源" }));
  expect((await screen.findByRole("alert")).textContent).toContain("来源报表暂不可用");
  expect(screen.queryByText("普通剧集")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "热门视频" }));
  expect(await screen.findByText("普通剧集")).toBeTruthy();
  expect(screen.queryByRole("alert")).toBeNull();
  expect(api.breakdown).toHaveBeenCalledTimes(2);
  fireEvent.click(screen.getByRole("button", { name: "刷新此报表" }));
  fireEvent.click(screen.getByRole("button", { name: "刷新此报表" }));
  expect((await screen.findByRole("alert")).textContent).toContain("保留上次成功");
  expect(screen.getByText("普通剧集")).toBeTruthy();
  expect(api.breakdown).toHaveBeenCalledTimes(3);
});

it("ignores late tab responses and discards cached reports on range or revision changes", async () => {
  const api = commands();
  let resolveOld!: (result: AnalyticsBreakdown) => void;
  vi.mocked(api.breakdown).mockReturnValueOnce(new Promise(resolve => { resolveOld = resolve; }));
  const { rerender, props } = setup(api);
  fireEvent.click(screen.getByRole("button", { name: "观看设备" }));
  await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
  await act(async () => resolveOld({ ...base, rows: [{ ...base.rows[0], title: "过时标题" }] }));
  fireEvent.click(screen.getByRole("button", { name: "热门视频" }));
  expect(await screen.findByText("普通剧集")).toBeTruthy();
  expect(screen.queryByText("过时标题")).toBeNull();
  expect(api.breakdown).toHaveBeenCalledTimes(3);
  vi.mocked(api.breakdown).mockReturnValueOnce(new Promise(() => {}));
  rerender(<AnalyticsDetails {...props} endDate="2026-09-12" revision="new" />);
  expect(screen.queryByText("普通剧集")).toBeNull();
  expect(api.breakdown).toHaveBeenLastCalledWith("c1", "2026-09-01", "2026-09-12", "videos", undefined);
});

it("omits unsupported traffic averages and uses only returned views for shares", async () => {
  const api = commands();
  vi.mocked(api.breakdown).mockResolvedValue({ ...base, kind: "traffic", rows: [
    { ...base.rows[0], key: "SHORTS", views: 30 }, { ...base.rows[0], key: "YT_SEARCH", views: 70 },
  ] });
  setup(api);
  fireEvent.click(screen.getByRole("button", { name: "流量来源" }));
  expect(await screen.findByText("Shorts 信息流")).toBeTruthy();
  expect(screen.getByText("YouTube 搜索")).toBeTruthy();
  expect(screen.getByText("30%")).toBeTruthy();
  expect(screen.getByText("70%")).toBeTruthy();
  expect(screen.queryByRole("columnheader", { name: "平均观看时长" })).toBeNull();
  expect(screen.getByText(/占比按此报表已返回的观看量计算/)).toBeTruthy();
});

it("preserves negative subscription trends and gaps without inventing zero data points", () => {
  const { container } = render(<AnalyticsTrend startDate="2026-09-01" endDate="2026-09-05" rows={[
    { ...metrics, date: "2026-09-01" }, { ...metrics, date: "2026-09-03", subscribersGained: null },
    { ...metrics, date: "2026-09-04", subscribersGained: 5 }, { ...metrics, date: "2026-09-05", subscribersGained: 6 },
  ]} />);
  fireEvent.change(screen.getByLabelText("趋势指标"), { target: { value: "netSubscribers" } });
  const chart = screen.getByRole("img");
  expect(within(chart).getByText("-2")).toBeTruthy();
  expect(container.querySelectorAll("circle")).toHaveLength(3);
  const path = container.querySelector("path")!.getAttribute("d")!;
  expect(path.match(/M /g)).toHaveLength(2);
  expect(path.match(/L /g)).toHaveLength(1);
  fireEvent.focus(screen.getByLabelText("2026-09-01：净增订阅 -2"));
  expect(screen.getByText(/2026-09-01 · 净增订阅：-2/, { selector: "p" })).toBeTruthy();
});
