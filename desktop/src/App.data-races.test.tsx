import { act, fireEvent, render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import type { DiscoveryPage, RankPage, SearchPage, SeriesItem } from "./types";

const apiMocks = vi.hoisted(() => ({
  chooseSaveDir: vi.fn(),
  downloadEpisode: vi.fn(),
  fetchCatalog: vi.fn(),
  fetchCategoryGroups: vi.fn(),
  fetchWebCategoryGroups: vi.fn(),
  fetchWebCategory: vi.fn(),
  fetchDiscovery: vi.fn(),
  fetchDiscoveryByCategory: vi.fn(),
  fetchDiscoveryMore: vi.fn(),
  fetchHealth: vi.fn(),
  fetchRank: vi.fn(),
  fetchSearch: vi.fn(),
  fetchSearchAll: vi.fn(),
  fetchSeriesMetrics: vi.fn(),
  fetchNewReleases: vi.fn(),
  getAiComponents: vi.fn(),
  getSettings: vi.fn(),
  installAiComponent: vi.fn(),
  removeAiComponent: vi.fn(),
  subscribeAiComponentProgress: vi.fn(),
  updateSettings: vi.fn(),
  getSaveDir: vi.fn(),
  openSaveDir: vi.fn(),
  revealPath: vi.fn(),
}));

vi.mock("./api", () => apiMocks);
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => undefined) }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
}

function memoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() {
      return values.size;
    },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => [...values.keys()][index] ?? null,
    removeItem: (key) => values.delete(key),
    setItem: (key, value) => values.set(key, value),
  };
}

function series(bookId: string, title: string): SeriesItem {
  return {
    bookId,
    seriesId: bookId,
    title,
    cover: "",
    firstVid: "",
    contentTypeCode: 1,
    episodeCount: 1,
    abstract: "",
    score: "",
    category: "真人剧",
    author: "",
    rankTags: [],
  };
}

function discoveryPage(item: SeriesItem): DiscoveryPage {
  return {
    items: [item],
    cellId: "discover-cell",
    nextOffset: 0,
    hasMore: false,
    sessionId: "discover-session",
    planId: "discover-plan",
    filterIds: "",
    selectedItems: "",
    categories: [],
    rankBoards: [],
  };
}

const rankItem = series("rank-book", "榜单结果短剧");
const searchItem = series("search-book", "搜索结果短剧");
const staleDiscoveryItem = series("discover-book", "过期发现结果");

const rankPage: RankPage = {
  items: [rankItem],
  nextCursor: "",
  hasMore: false,
  board: "ranklist_hot_sc",
  boardName: "推荐榜",
  releaseType: "all",
  boards: [{ id: "ranklist_hot_sc", name: "推荐榜" }],
};

const searchPage: SearchPage = {
  items: [searchItem],
  hasMore: false,
  nextOffset: 10,
  nextPassback: "10",
};

describe("App request ordering", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    Object.defineProperty(window, "localStorage", { configurable: true, value: memoryStorage() });
    window.history.replaceState({}, "", "/");
    apiMocks.fetchCatalog.mockResolvedValue([]);
    apiMocks.fetchCategoryGroups.mockResolvedValue([]);
    apiMocks.fetchWebCategoryGroups.mockResolvedValue([]);
    apiMocks.fetchWebCategory.mockResolvedValue({ items: [], nextPage: 2, hasMore: false });
    apiMocks.fetchHealth.mockResolvedValue({ status: "ok", pool_size: 1, active_count: 1 });
    apiMocks.getAiComponents.mockResolvedValue([]);
    apiMocks.subscribeAiComponentProgress.mockResolvedValue(() => undefined);
    apiMocks.getSettings.mockResolvedValue({ version: 1, saveDir: "/tmp/downloads", definition: "auto", notifyDownloadComplete: true, notifyNewReleases: true });
    apiMocks.updateSettings.mockImplementation(async (patch) => ({ version: 1, saveDir: "/tmp/downloads", definition: "auto", notifyDownloadComplete: true, notifyNewReleases: true, ...patch }));
    apiMocks.fetchNewReleases.mockResolvedValue({ items: [], nextCursor: "", hasMore: false, date: "2026-09-02", refreshedAt: "" });
    apiMocks.getSaveDir.mockResolvedValue("/tmp/downloads");
    apiMocks.fetchRank.mockResolvedValue(rankPage);
    apiMocks.fetchSearch.mockResolvedValue(searchPage);
    apiMocks.fetchSearchAll.mockResolvedValue(searchPage);
    apiMocks.fetchSeriesMetrics.mockResolvedValue({
      seriesId: "default",
      contentTypeCode: 1,
    });
  });

  it("does not let a slower discovery request overwrite a selected rank page", async () => {
    const discovery = deferred<DiscoveryPage>();
    apiMocks.fetchDiscovery.mockReturnValue(discovery.promise);
    const view = render(<App />);

    await waitFor(() => expect(apiMocks.fetchDiscovery).toHaveBeenCalledTimes(2));
    fireEvent.click(view.getByRole("button", { name: "榜单" }));
    await waitFor(() => expect(view.getAllByText("榜单结果短剧").length).toBeGreaterThan(0));

    await act(async () => discovery.resolve(discoveryPage(staleDiscoveryItem)));

    expect(view.getAllByText("榜单结果短剧").length).toBeGreaterThan(0);
    expect(view.queryByText("过期发现结果")).toBeNull();
  });

  it("allows episode selection while metrics are still pending", async () => {
    apiMocks.fetchDiscovery.mockResolvedValue(discoveryPage(series("fast-catalog", "快速目录")));
    apiMocks.fetchCatalog.mockResolvedValue([{ itemId: "episode-fast", index: 1, title: "第1集" }]);
    apiMocks.fetchSeriesMetrics.mockReturnValue(new Promise(() => {}));
    const view = render(<App />);
    await waitFor(() => expect(view.getByRole("button", { name: "加入下载队列（已选 1 集）" })).toHaveProperty("disabled", false));
    expect(view.getByText("正在加载剧集数据…")).toBeTruthy();
  });

  it("does not let a slower discovery request overwrite submitted search results", async () => {
    const discovery = deferred<DiscoveryPage>();
    apiMocks.fetchDiscovery.mockReturnValue(discovery.promise);
    const view = render(<App />);

    await waitFor(() => expect(apiMocks.fetchDiscovery).toHaveBeenCalledTimes(2));
    const input = view.getByRole("textbox", { name: "搜索短剧或漫剧" });
    fireEvent.change(input, { target: { value: "搜索词" } });
    fireEvent.submit(input.closest("form")!);
    await waitFor(() => expect(view.getAllByText("搜索结果短剧").length).toBeGreaterThan(0));

    await act(async () => discovery.resolve(discoveryPage(staleDiscoveryItem)));

    expect(view.getAllByText("搜索结果短剧").length).toBeGreaterThan(0);
    expect(view.queryByText("过期发现结果")).toBeNull();
  });

  it("does not let metrics from a previously selected series overwrite the current series", async () => {
    const first = series("first-book", "第一部短剧");
    const second = series("second-book", "第二部短剧");
    apiMocks.fetchDiscovery.mockResolvedValue({ ...discoveryPage(first), items: [first, second] });
    const firstMetrics = deferred<{ seriesId: string; contentTypeCode: number; playCount: number }>();
    apiMocks.fetchSeriesMetrics
      .mockReturnValueOnce(firstMetrics.promise)
      .mockResolvedValueOnce({ seriesId: second.seriesId, contentTypeCode: 1, playCount: 22 });

    const view = render(<App />);
    await waitFor(() => expect(view.getAllByText("第二部短剧").length).toBeGreaterThan(0));
    fireEvent.click(view.getByRole("button", { name: /第二部短剧/ }));
    await waitFor(() => expect(view.getByTestId("metric-play").textContent).toBe("播放量 22"));

    await act(async () => firstMetrics.resolve({ seriesId: first.seriesId, contentTypeCode: 1, playCount: 11 }));

    expect(view.getByTestId("metric-play").textContent).toBe("播放量 22");
  });
});
