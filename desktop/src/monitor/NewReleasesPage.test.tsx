import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { SeriesItem } from "../types";
import { NewReleasesPage } from "./NewReleasesPage";

const item: SeriesItem = {
  bookId: "new-a", seriesId: "new-a", title: "今日新剧", cover: "", firstVid: "", contentTypeCode: 1,
  episodeCount: 20, abstract: "", score: "", category: "校园", author: "", rankTags: [],
  onlineTime: 1_788_408_000, playCount: 0, hotCount: 25_000, collectCount: undefined, likeCount: 8,
};

function model(overrides = {}) {
  return {
    type: "playlet" as const, items: [item], filteredItems: [item], categories: ["校园", "古风", "其他"],
    selectedCategory: "", loading: false, error: "", refreshedAt: "2026-09-03T09:30:00+08:00",
    unseenCount: 0, hasMore: false, setType: vi.fn(), setCategory: vi.fn(), refresh: vi.fn(),
    loadMore: vi.fn(), clearUnseen: vi.fn(), ...overrides,
  };
}

describe("NewReleasesPage", () => {
  it("renders exactly three monitor types and detailed category filters", () => {
    const state = model();
    const view = render(<NewReleasesPage model={state} onSelect={vi.fn()} />);

    fireEvent.click(view.getByRole("button", { name: "AI剧" }));
    expect(state.setType).toHaveBeenCalledWith("ai_playlet");
    expect(view.queryByRole("button", { name: "全部类型" })).toBeNull();
    expect(view.queryByRole("button", { name: "改编剧" })).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "古风" }));
    expect(state.setCategory).toHaveBeenCalledWith("古风");
    expect(view.getByText((_, element) => element?.tagName === "P" && element.textContent?.includes("已收录 1 部") === true)).toBeTruthy();
  });

  it("renders filtered rows, Shanghai time and missing metrics", () => {
    const state = model();
    const view = render(<NewReleasesPage model={state} onSelect={vi.fn()} />);

    expect(view.getByText(/上线 \d{2}:\d{2}/)).toBeTruthy();
    expect(view.getByText("播放 0")).toBeTruthy();
    expect(view.getByText("热度 2.5万")).toBeTruthy();
    expect(view.getByText("收藏 —")).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "立即刷新" }));
    expect(state.refresh).toHaveBeenCalled();
  });

  it("shows an empty-today state only after scanning completes", () => {
    const view = render(<NewReleasesPage model={model({ items: [], filteredItems: [], categories: [] })} onSelect={vi.fn()} />);
    expect(view.getByText("今天还没有新上线剧目")).toBeTruthy();

    view.rerender(<NewReleasesPage model={model({ items: [], filteredItems: [], categories: [], loading: true })} onSelect={vi.fn()} />);
    expect(view.queryByText("今天还没有新上线剧目")).toBeNull();
    expect(view.getByText("正在扫描全部今日新剧…")).toBeTruthy();
  });
});
