import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { SeriesItem } from "../types";
import type { NewReleaseMonitor } from "./useNewReleaseMonitor";
import { NewReleasesPage } from "./NewReleasesPage";

const item: SeriesItem = {
  bookId: "new-a", seriesId: "new-a", title: "今日新剧", cover: "", firstVid: "", contentTypeCode: 1,
  episodeCount: 20, abstract: "", score: "", category: "校园", author: "", rankTags: [],
  onlineTime: Date.parse("2026-09-03T09:20:00+08:00") / 1000, playCount: 0, hotCount: 25_000, collectCount: undefined, likeCount: 8, commentCount: 42,
};

function model(overrides: Partial<NewReleaseMonitor> = {}): NewReleaseMonitor {
  return {
    type: "playlet", date: "2026-09-03", dateScope: "today", source: "subscribe",
    items: [item], filteredItems: [item], categories: ["校园", "古风", "其他"],
    selectedCategory: "", loading: false, error: "", refreshedAt: "2026-09-03T09:30:00+08:00",
    query: "", sort: "latest", scanPages: 2, scanComplete: true,
    unseenCount: 0, hasMore: false, setType: vi.fn(), setCategory: vi.fn(), setQuery: vi.fn(), setSort: vi.fn(), refresh: vi.fn(),
    loadMore: vi.fn(), clearUnseen: vi.fn(), ...overrides,
  };
}

describe("NewReleasesPage", () => {
  it("renders three types, detailed categories and scan status", () => {
    const state = model();
    const view = render(<NewReleasesPage model={state} onSelect={vi.fn()} detectOrientation={false} />);
    fireEvent.click(view.getByRole("button", { name: "AI剧" }));
    expect(state.setType).toHaveBeenCalledWith("ai_playlet");
    expect(view.queryByRole("button", { name: "全部类型" })).toBeNull();
    expect(view.queryByRole("button", { name: "改编剧" })).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "古风" }));
    expect(state.setCategory).toHaveBeenCalledWith("古风");
    expect(view.getByText(/已收录 1 部/)).toBeTruthy();
    expect(view.getByText("检查完成")).toBeTruthy();
    expect(view.getByText("已检查 2 页")).toBeTruthy();
    expect(view.getByText("2026-09-03 · 北京时间")).toBeTruthy();
  });

  it("renders Shanghai time, real zero counts and missing metrics", () => {
    const state = model();
    const view = render(<NewReleasesPage model={state} onSelect={vi.fn()} detectOrientation={false} />);
    expect(view.getByText("上线 09:20")).toBeTruthy();
    expect(view.getByText("真人剧 · 20 集 · 校园")).toBeTruthy();
    expect(view.getByText("播放 0")).toBeTruthy();
    expect(view.getByText("🔥 热度 2.5万")).toBeTruthy();
    expect(view.getByText("收藏 —")).toBeTruthy();
    expect(view.getByText("讨论 42")).toBeTruthy();
    expect(view.queryByText("点赞 8")).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "立即刷新" }));
    expect(state.refresh).toHaveBeenCalled();
  });

  it("shows an empty-today state only after a successful complete scan", () => {
    const state = model({ items: [], filteredItems: [], categories: [] });
    const view = render(<NewReleasesPage model={state} onSelect={vi.fn()} detectOrientation={false} />);
    expect(view.getByText("今天还没有新上线剧目")).toBeTruthy();
    view.rerender(<NewReleasesPage model={{ ...state, loading: true, scanComplete: false, scanPages: 0 }} onSelect={vi.fn()} detectOrientation={false} />);
    expect(view.queryByText("今天还没有新上线剧目")).toBeNull();
    expect(view.getByText(/正在扫描今日上新列表/)).toBeTruthy();
    view.rerender(<NewReleasesPage model={{ ...state, error: "第一页加载失败", scanComplete: false }} onSelect={vi.fn()} detectOrientation={false} />);
    expect(view.queryByText("今天还没有新上线剧目")).toBeNull();
    expect(view.queryByText(/已完整检查/)).toBeNull();
    expect(view.getByRole("alert").textContent).toContain("第一页加载失败");
    expect(view.getByText("检查未完成")).toBeTruthy();
  });

  it("explains latest feeds and uses the correct scan text", () => {
    const state = model({ type: "ai_playlet", dateScope: "latest", source: "rank", items: [], filteredItems: [], categories: [] });
    const view = render(<NewReleasesPage model={state} onSelect={vi.fn()} detectOrientation={false} />);
    expect(view.getByText(/按新剧榜最新收录展示/)).toBeTruthy();
    expect(view.getByText("当前暂无可用新剧")).toBeTruthy();
    view.rerender(<NewReleasesPage model={{ ...state, loading: true }} onSelect={vi.fn()} detectOrientation={false} />);
    expect(view.getByText(/正在扫描新剧榜最新收录/)).toBeTruthy();
    expect(view.queryByText(/正在扫描今日上新列表/)).toBeNull();
  });

  it("shows earlier dates and handles invalid timestamps and metrics", () => {
    const older = { ...item, onlineTime: Date.parse("2026-08-30T12:00:00+08:00") / 1000 };
    const state = model({ filteredItems: [older] });
    const view = render(<NewReleasesPage model={state} onSelect={vi.fn()} detectOrientation={false} />);
    expect(view.getByText("上线 2026-08-30 12:00")).toBeTruthy();
    view.rerender(<NewReleasesPage model={{ ...state, filteredItems: [{ ...item, onlineTime: Number.NaN, hotCount: Number.NaN }], refreshedAt: "invalid" }} onSelect={vi.fn()} detectOrientation={false} />);
    expect(view.getByText("上线时间未知")).toBeTruthy();
    expect(view.getByText("🔥 热度 —")).toBeTruthy();
    expect(view.getByText(/尚未完成检查/)).toBeTruthy();
  });

  it("connects title search, ordering and clearing empty filters", () => {
    const state = model({ filteredItems: [], query: "不存在", selectedCategory: "古风" });
    const view = render(<NewReleasesPage model={state} onSelect={vi.fn()} detectOrientation={false} />);
    fireEvent.change(view.getByRole("searchbox", { name: "搜索已收录剧名" }), { target: { value: "仙侠" } });
    expect(state.setQuery).toHaveBeenCalledWith("仙侠");
    fireEvent.change(view.getByRole("combobox", { name: "新剧排序" }), { target: { value: "hot" } });
    expect(state.setSort).toHaveBeenCalledWith("hot");
    fireEvent.click(view.getByRole("button", { name: "清除筛选" }));
    expect(state.setQuery).toHaveBeenLastCalledWith("");
    expect(state.setCategory).toHaveBeenLastCalledWith("");
  });

  it("bounds the initial category list while allowing every tag to be selected", () => {
    const state = model({ categories: Array.from({ length: 30 }, (_, index) => `题材${index}`) });
    const view = render(<NewReleasesPage model={state} onSelect={vi.fn()} detectOrientation={false} />);
    expect(view.queryByRole("button", { name: "题材29" })).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "更多分类（30）" }));
    fireEvent.click(view.getByRole("button", { name: "题材29" }));
    expect(state.setCategory).toHaveBeenCalledWith("题材29");
    fireEvent.click(view.getByRole("button", { name: "收起分类" }));
    expect(view.queryByRole("button", { name: "题材29" })).toBeNull();
  });
});
