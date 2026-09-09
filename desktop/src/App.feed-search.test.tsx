import { act, fireEvent, render, waitFor, within } from "@testing-library/react";
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

function memoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() { return values.size; },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => [...values.keys()][index] ?? null,
    removeItem: (key) => values.delete(key),
    setItem: (key, value) => values.set(key, value),
  };
}

function series(index: number, title = `资讯 ${index}`): SeriesItem {
  return {
    bookId: `book-${index}`,
    seriesId: `book-${index}`,
    title,
    cover: "",
    firstVid: "",
    contentTypeCode: 1,
    episodeCount: 1,
    abstract: "",
    score: "8.0",
    category: "真人剧",
    author: "",
    rankTags: [],
  };
}

function discoveryPage(items: SeriesItem[], nextOffset: number, hasMore: boolean): DiscoveryPage {
  return {
    items,
    cellId: "discover-cell",
    nextOffset,
    hasMore,
    sessionId: `discover-session-${nextOffset}`,
    planId: "discover-plan",
    filterIds: items.map((item) => item.bookId).join(","),
    selectedItems: "",
    categories: [],
    rankBoards: [],
  };
}

function rankPage(items: SeriesItem[], nextOffset: number, hasMore: boolean): RankPage {
  return {
    items,
    nextCursor: hasMore ? `rank-cursor-${nextOffset}` : "",
    hasMore,
    board: "ranklist_hot_sc",
    boardName: "推荐榜",
    releaseType: "all",
    boards: [
      { id: "ranklist_hot_sc", name: "推荐榜" },
      { id: "ranklist_hot_play_sc", name: "热播榜" },
      { id: "ranklist_prestige", name: "臻果榜" },
      { id: "ranklist_subscribe", name: "预约榜" },
      { id: "ranklist_new_rank_sc", name: "新剧榜" },
      { id: "ranklist_hot_search_sc", name: "热搜榜" },
      { id: "ranklist_must_watch", name: "必看榜" },
      { id: "ranklist_followed", name: "收藏榜" },
    ],
  };
}

describe("App feed and search controls", () => {
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
    apiMocks.fetchSeriesMetrics.mockResolvedValue({ seriesId: "", contentTypeCode: 1 });
    apiMocks.getSaveDir.mockResolvedValue("/tmp/downloads");
    apiMocks.fetchDiscovery.mockResolvedValue(discoveryPage([series(1)], 0, false));
    apiMocks.fetchDiscoveryByCategory.mockResolvedValue(discoveryPage([series(1)], 0, false));
    apiMocks.fetchDiscoveryMore.mockResolvedValue(discoveryPage([], 0, false));
    apiMocks.fetchRank.mockResolvedValue(rankPage([series(1)], 0, false));
    apiMocks.fetchSearch.mockResolvedValue({ items: [], hasMore: false, nextOffset: 0, nextPassback: "" });
    apiMocks.fetchSearchAll.mockResolvedValue({ items: [], hasMore: false, nextOffset: 0, nextPassback: "" });
  });

  it("opens combined real-drama categories and switches back to the manju video feed", async () => {
    apiMocks.fetchWebCategoryGroups.mockResolvedValue([{ id: "background", name: "背景", items: [{ id: "", name: "全部" }, { id: "cate_757", name: "现代" }] }]);
    apiMocks.fetchWebCategory.mockResolvedValue({ items: [series(888, "分类真人剧")], nextPage: 2, hasMore: false });
    const view = render(<App />);
    fireEvent.click(view.getByRole("button", { name: "分类浏览" }));
    await waitFor(() => expect(view.getAllByText("分类真人剧").length).toBeGreaterThan(0));
    expect(apiMocks.fetchWebCategory).toHaveBeenCalledWith("drama", expect.any(Object), 1);
    fireEvent.click(view.getByRole("button", { name: "现代" }));
    await waitFor(() => expect(apiMocks.fetchWebCategory).toHaveBeenLastCalledWith("drama", expect.objectContaining({ background: "cate_757" }), 1));
    expect(apiMocks.fetchCatalog).toHaveBeenCalledWith("book-888");
    fireEvent.click(within(view.container.querySelector(".content-segment") as HTMLElement).getByRole("button", { name: "漫剧" }));
    await waitFor(() => expect(apiMocks.fetchDiscovery).toHaveBeenLastCalledWith("manju"));
    expect(view.getByRole("heading", { name: "分类浏览" })).toBeTruthy();
  });

  it("labels 1004 comics and shows heat on home cards", async () => {
    apiMocks.fetchDiscovery.mockResolvedValue(discoveryPage([{ ...series(777, "真实漫剧"), contentTypeCode: 1004, category: "", hotCount: 12500 }], 0, false));
    const view = render(<App />);
    await waitFor(() => expect(view.container.querySelector(".poster-copy")?.textContent).toContain("漫剧"));
    expect(view.container.querySelector(".poster-copy")?.textContent).toContain("🔥 热度 1.3万");
  });

  it("opens AI recommendations and keeps category browsing available", async () => {
    apiMocks.fetchRank.mockResolvedValue(rankPage([{ ...series(778, "AI推荐剧"), releaseType: "ai_playlet", category: "科幻末世" }], 0, false));
    const view = render(<App />);
    fireEvent.click(within(view.container.querySelector(".content-segment") as HTMLElement).getByRole("button", { name: "AI剧" }));
    await waitFor(() => expect(view.getAllByText("AI推荐剧").length).toBeGreaterThan(0));
    fireEvent.click(view.getByRole("button", { name: "分类浏览" }));
    await waitFor(() => expect(view.getByRole("button", { name: "科幻末世" })).toBeTruthy());
    expect(view.getByRole("heading", { name: "分类浏览" })).toBeTruthy();
  });

  it("restores search type selection after leaving AI home", async () => {
    const view = render(<App />);
    const types = within(view.container.querySelector(".content-segment") as HTMLElement);
    fireEvent.click(types.getByRole("button", { name: "AI剧" }));
    await waitFor(() => expect(view.container.querySelector(".category-browser .poster-card")).not.toBeNull());
    await act(async () => {
      fireEvent.click(within(view.getByRole("navigation", { name: "主导航" })).getByRole("button", { name: "搜索" }));
    });
    expect(types.getByRole("button", { name: "全部" }).getAttribute("aria-pressed")).toBe("true");
  });

  it("shows search modes and keeps only an exact title in matching mode", async () => {
    const searchResult: SearchPage = {
      items: [series(101, "天下第一纨绔"), series(102, "天下第一纨绔3")],
      hasMore: false,
      nextOffset: 0,
      nextPassback: "",
    };
    apiMocks.fetchSearchAll.mockResolvedValue(searchResult);
    const view = render(<App />);

    const form = view.container.querySelector(".search-form");
    expect(form).not.toBeNull();
    const search = within(form as HTMLElement);
    expect(search.getByRole("button", { name: "模糊识别" })).toBeTruthy();
    const contentSegment = view.container.querySelector(".content-segment");
    expect(contentSegment).not.toBeNull();
    expect(within(contentSegment as HTMLElement).getByRole("button", { name: "真人剧" })).toBeTruthy();
    expect(within(contentSegment as HTMLElement).getByRole("button", { name: "漫剧" })).toBeTruthy();
    expect(within(contentSegment as HTMLElement).getByRole("button", { name: "全部" })).toBeTruthy();
    expect(within(contentSegment as HTMLElement).getByRole("button", { name: "AI剧" })).toHaveProperty("disabled", false);
    fireEvent.click(search.getByRole("button", { name: "匹配识别" }));
    fireEvent.change(search.getByRole("textbox", { name: "搜索短剧或漫剧" }), { target: { value: "天下第一纨绔" } });
    fireEvent.click(search.getByRole("button", { name: "搜索" }));

    await waitFor(() => expect(view.getAllByText("天下第一纨绔").length).toBeGreaterThan(0));
    expect(view.container.querySelectorAll(".poster-card")).toHaveLength(1);
    expect(view.queryByText("天下第一纨绔3")).toBeNull();
  });

  it("opens search immediately from rank and restores cached results after visiting settings", async () => {
    let resolve!: (page: SearchPage) => void;
    apiMocks.fetchSearchAll.mockReturnValue(new Promise((done) => { resolve = done; }));
    const view = render(<App />);
    const navigation = within(view.getByRole("navigation", { name: "主导航" }));
    fireEvent.click(navigation.getByRole("button", { name: "榜单" }));
    await waitFor(() => expect(apiMocks.fetchRank).toHaveBeenCalledWith(expect.objectContaining({ type: "all" })));
    const input = view.getByRole("textbox", { name: "搜索短剧或漫剧" });
    fireEvent.change(input, { target: { value: "缓存剧" } });
    fireEvent.submit(input.closest("form")!);
    expect(view.getByRole("heading", { name: "搜索结果" })).toBeTruthy();
    expect(view.getByRole("status", { name: "正在加载内容" })).toBeTruthy();
    await waitFor(() => expect(apiMocks.fetchSearchAll).toHaveBeenCalledTimes(1));
    await act(async () => resolve({ items: [series(501, "缓存剧")], hasMore: false, nextOffset: 0, nextPassback: "" }));
    await waitFor(() => expect(view.getAllByText("缓存剧").length).toBeGreaterThan(0));
    fireEvent.click(navigation.getByRole("button", { name: "设置" }));
    expect(view.getByRole("heading", { name: "设置" })).toBeTruthy();
    fireEvent.click(navigation.getByRole("button", { name: "搜索" }));
    await waitFor(() => expect(view.getAllByText("缓存剧").length).toBeGreaterThan(0));
    expect(apiMocks.fetchSearchAll).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(view.getByRole("textbox", { name: "搜索短剧或漫剧" }));
    expect(view.queryByRole("status", { name: "正在加载内容" })).toBeNull();
  });

  it("defaults to all and does not use a new input draft for pagination", async () => {
    apiMocks.fetchSearchAll.mockImplementation(async (_query, cursor) => ({
      items: cursor ? [series(999, "第二页")] : Array.from({ length: 20 }, (_, i) => series(600 + i, `原词 ${i}`)),
      hasMore: !cursor, nextOffset: 20, nextPassback: cursor ? "" : "next",
    }));
    const view = render(<App />);
    const types = within(view.container.querySelector(".content-segment") as HTMLElement);
    expect(types.getByRole("button", { name: "全部" }).getAttribute("aria-pressed")).toBe("true");
    const input = view.getByRole("textbox", { name: "搜索短剧或漫剧" });
    fireEvent.change(input, { target: { value: "原词" } }); fireEvent.submit(input.closest("form")!);
    await waitFor(() => expect(view.container.querySelectorAll(".poster-card")).toHaveLength(20));
    fireEvent.change(input, { target: { value: "尚未提交" } });
    fireEvent.click(view.getByRole("button", { name: "加载更多" }));
    await waitFor(() => expect(apiMocks.fetchSearchAll).toHaveBeenLastCalledWith("原词", "next"));
    expect(view.getByText("第二页")).toBeTruthy();
  });

  it("searches both supported content types from the all option", async () => {
    apiMocks.fetchSearchAll.mockResolvedValue({
      items: [series(301, "全部搜索结果")],
      hasMore: false,
      nextOffset: 0,
      nextPassback: "",
    });
    const view = render(<App />);
    const form = view.container.querySelector(".search-form") as HTMLElement;
    const contentSegment = view.container.querySelector(".content-segment") as HTMLElement;
    fireEvent.click(within(contentSegment).getByRole("button", { name: "全部" }));
    fireEvent.change(within(form).getByRole("textbox", { name: "搜索短剧或漫剧" }), { target: { value: "全部搜索" } });
    fireEvent.click(within(form).getByRole("button", { name: "搜索" }));

    await waitFor(() => expect(apiMocks.fetchSearchAll).toHaveBeenCalledWith("全部搜索", ""));
    expect(view.getAllByText("全部搜索结果").length).toBeGreaterThan(0);
  });

  it("shows a visible loading layer while a search replaces existing cards", async () => {
    let resolveSearch!: (value: SearchPage) => void;
    const pending = new Promise<SearchPage>((resolve) => { resolveSearch = resolve; });
    apiMocks.fetchSearchAll.mockReturnValue(pending);
    const view = render(<App />);
    await waitFor(() => expect(view.container.querySelectorAll(".poster-card")).toHaveLength(1));

    const form = view.container.querySelector(".search-form") as HTMLElement;
    fireEvent.change(within(form).getByRole("textbox", { name: "搜索短剧或漫剧" }), { target: { value: "新剧" } });
    fireEvent.click(within(form).getByRole("button", { name: "搜索" }));

    expect(view.getByRole("status", { name: "正在加载内容" })).toBeTruthy();
    await waitFor(() => expect(view.container.querySelectorAll(".poster-card")).toHaveLength(0));
    resolveSearch({ items: [series(2, "新剧")], hasMore: false, nextOffset: 0, nextPassback: "" });
    await waitFor(() => expect(view.getAllByText("新剧").length).toBeGreaterThan(0));
  });

  it("shows the first home page without waiting for categories or extra pages", async () => {
    apiMocks.fetchCategoryGroups.mockReturnValue(new Promise(() => {}));
    apiMocks.fetchDiscovery.mockResolvedValue(discoveryPage(
      Array.from({ length: 6 }, (_, index) => series(index + 1)),
      6,
      true,
    ));
    apiMocks.fetchDiscoveryMore
      .mockResolvedValueOnce(discoveryPage(Array.from({ length: 6 }, (_, index) => series(index + 7)), 12, true))
      .mockResolvedValueOnce(discoveryPage(Array.from({ length: 6 }, (_, index) => series(index + 13)), 18, true))
      .mockResolvedValueOnce(discoveryPage(Array.from({ length: 6 }, (_, index) => series(index + 19)), 24, true))
      .mockResolvedValueOnce(discoveryPage(Array.from({ length: 6 }, (_, index) => series(index + 25)), 30, true))
      .mockResolvedValueOnce(discoveryPage(Array.from({ length: 6 }, (_, index) => series(index + 31)), 36, true))
      .mockResolvedValueOnce(discoveryPage(Array.from({ length: 6 }, (_, index) => series(index + 37)), 42, false));
    const view = render(<App />);

    await waitFor(() => expect(view.container.querySelectorAll(".poster-card")).toHaveLength(6));
    expect(apiMocks.fetchDiscoveryMore).not.toHaveBeenCalled();
    expect(view.queryByRole("status", { name: "正在加载内容" })).toBeNull();
    expect(view.getByRole("heading", { name: "首页推荐" })).toBeTruthy();
    expect(view.queryByText("资讯 21")).toBeNull();

    const scroller = view.container.querySelector(".library-main") as HTMLElement;
    Object.defineProperties(scroller, {
      clientHeight: { configurable: true, value: 400 },
      scrollHeight: { configurable: true, value: 1200 },
      scrollTop: { configurable: true, value: 760 },
    });
    fireEvent.scroll(scroller);

    await waitFor(() => expect(view.container.querySelectorAll(".poster-card")).toHaveLength(26));
    expect(view.getAllByText("资讯 26").length).toBeGreaterThan(0);
  });

  it("numbers twenty initial rank items and auto-loads the next page near the bottom", async () => {
    const first = rankPage(Array.from({ length: 10 }, (_, index) => series(index + 1)), 10, true);
    const second = rankPage(Array.from({ length: 10 }, (_, index) => series(index + 11)), 20, true);
    const third = rankPage(Array.from({ length: 20 }, (_, index) => series(index + 21)), 40, false);
    apiMocks.fetchRank.mockResolvedValueOnce(first).mockResolvedValueOnce(second).mockResolvedValueOnce(third);
    const view = render(<App />);
    fireEvent.click(view.getByRole("button", { name: "榜单" }));

    await waitFor(() => expect(view.container.querySelectorAll(".poster-card")).toHaveLength(20));
    expect(view.getByText("NO.1")).toBeTruthy();
    expect(view.getByText("NO.20")).toBeTruthy();

    const scroller = view.container.querySelector(".library-main") as HTMLElement;
    Object.defineProperties(scroller, {
      clientHeight: { configurable: true, value: 400 },
      scrollHeight: { configurable: true, value: 1200 },
      scrollTop: { configurable: true, value: 760 },
    });
    fireEvent.scroll(scroller);

    await waitFor(() => expect(view.container.querySelectorAll(".poster-card")).toHaveLength(40));
    expect(view.getByText("NO.40")).toBeTruthy();
  });

  it("requests the selected hot-search board for the manju rank", async () => {
    const view = render(<App />);
    fireEvent.click(view.getByRole("button", { name: "榜单" }));
    await waitFor(() => expect(apiMocks.fetchRank).toHaveBeenCalled());

    fireEvent.click(view.getByRole("button", { name: "漫剧" }));
    await waitFor(() => expect(apiMocks.fetchRank.mock.lastCall?.[0]).toMatchObject({ type: "comic_series_rank" }));

    fireEvent.click(view.getByRole("button", { name: "热搜榜" }));
    await waitFor(() => expect(apiMocks.fetchRank.mock.lastCall?.[0]).toMatchObject({
      board: "ranklist_hot_search_sc",
      type: "comic_series_rank",
    }));
  });

  it("continues rank numbering past NO.100 until upstream ends", async () => {
    for (let pageIndex = 0; pageIndex < 12; pageIndex += 1) {
      const start = pageIndex * 10 + 1;
      apiMocks.fetchRank.mockResolvedValueOnce(rankPage(
        Array.from({ length: 10 }, (_, index) => series(start + index)),
        start + 9,
        pageIndex < 11,
      ));
    }
    const view = render(<App />);
    fireEvent.click(view.getByRole("button", { name: "榜单" }));
    await waitFor(() => expect(view.getByText("NO.20")).toBeTruthy());

    const scroller = view.container.querySelector(".library-main") as HTMLElement;
    Object.defineProperties(scroller, {
      clientHeight: { configurable: true, value: 400 },
      scrollHeight: { configurable: true, value: 1200 },
      scrollTop: { configurable: true, value: 760 },
    });
    for (const count of [40, 60, 80, 100, 120]) {
      fireEvent.scroll(scroller);
      await waitFor(() => expect(view.getByText(`NO.${count}`)).toBeTruthy());
    }
    expect(view.getByText("NO.101")).toBeTruthy();
    expect(view.queryByText("范围")).toBeNull();
    for (const label of ["推荐榜", "热播榜", "臻果榜", "预约榜", "新剧榜", "热搜榜", "必看榜", "收藏榜"]) {
      expect(view.getByRole("button", { name: label })).toBeTruthy();
    }
    expect(apiMocks.fetchRank).toHaveBeenCalledTimes(12);
  });
});
