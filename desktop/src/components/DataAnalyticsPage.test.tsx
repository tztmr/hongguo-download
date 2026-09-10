import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { previewYouTubeModel } from "../preview";
import type { AnalyticsCommands } from "../youtube/analyticsCommands";
import { DataAnalyticsPage } from "./DataAnalyticsPage";

afterEach(() => vi.restoreAllMocks());

function commands(): AnalyticsCommands {
  return {
    snapshot: vi.fn().mockResolvedValue({ channelId: "UC_PREVIEW", viewCount: "123456", fetchedAt: "2026-09-10T10:00:00Z" }),
    report: vi.fn().mockResolvedValue({
      channelId: "UC_PREVIEW", startDate: "2026-08-15", endDate: "2026-09-10", returnedEndDate: "2026-09-09",
      views: 3210, estimatedMinutesWatched: 987.5, averageViewDuration: 42, fetchedAt: "2026-09-10T10:00:00Z",
      timezone: "America/Los_Angeles", rows: [{ date: "2026-09-09", views: 3210, estimatedMinutesWatched: 987.5, averageViewDuration: 42 }],
    }),
  };
}

describe("DataAnalyticsPage", () => {
  it("loads cumulative and historical channel statistics with the selected range", async () => {
    const api = commands();
    render(<DataAnalyticsPage youtube={previewYouTubeModel} commands={api} />);

    expect(await screen.findByText("123,456")).toBeTruthy();
    expect(screen.getAllByText("3,210")).toHaveLength(2);
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
