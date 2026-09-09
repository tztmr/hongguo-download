import { act, fireEvent, render, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { SeriesItem, WebCategoryPage } from "../types";
import { CategoryBrowser, DEFAULT_CATEGORY_FILTERS } from "./CategoryBrowser";
const groups = [
  { id: "background", name: "背景", items: [{ id: "", name: "全部" }, { id: "cate_757", name: "现代" }] },
  { id: "topic", name: "主题", items: [{ id: "", name: "全部" }, { id: "cate_1021", name: "脑洞" }] },
];
function item(id: string): SeriesItem { return { bookId: id, seriesId: id, title: id, firstVid: "", cover: "", contentTypeCode: 1, episodeCount: 1, category: "", abstract: "", score: "", author: "", rankTags: [] }; }
function page(ids: string[], nextPage = 2, hasMore = false): WebCategoryPage { return { items: ids.map(item), nextPage, hasMore, total: 100 }; }
function api() { return { fetchWebCategoryGroups: vi.fn().mockResolvedValue(groups), fetchWebCategory: vi.fn().mockResolvedValue(page(["默认剧"])) }; }
function deferred() { let resolve!: (value: WebCategoryPage) => void; return { promise: new Promise<WebCategoryPage>((done) => { resolve = done; }), resolve: (value: WebCategoryPage) => resolve(value) }; }
describe("CategoryBrowser", () => {
  it("combines facets, restarts at page one, and retains filters through pagination", async () => {
    const mock = api();
    mock.fetchWebCategory.mockImplementation(async (_type, _filters, number) => page(["筛选剧"], number + 1, true));
    const view = render(<CategoryBrowser api={mock} onSelect={vi.fn()} detectOrientation={false} />);
    fireEvent.click(await view.findByRole("button", { name: "现代" }));
    fireEvent.click(view.getByRole("button", { name: "脑洞" }));
    await waitFor(() => expect(mock.fetchWebCategory).toHaveBeenLastCalledWith("drama", { ...DEFAULT_CATEGORY_FILTERS, background: "cate_757", topic: "cate_1021" }, 1));
    fireEvent.click(await view.findByRole("button", { name: "加载更多" }));
    await waitFor(() => expect(mock.fetchWebCategory).toHaveBeenLastCalledWith("drama", expect.objectContaining({ background: "cate_757", topic: "cate_1021" }), 2));
    fireEvent.click(view.getByRole("button", { name: "清空筛选" }));
    await waitFor(() => expect(mock.fetchWebCategory).toHaveBeenLastCalledWith("drama", DEFAULT_CATEGORY_FILTERS, 1));
  });
  it("ignores old pages after filters change and deduplicates later pages", async () => {
    const stale = deferred(); const mock = api();
    mock.fetchWebCategory.mockResolvedValueOnce(page(["旧剧"], 2, true)).mockReturnValueOnce(stale.promise).mockResolvedValueOnce(page(["新剧"], 2, true)).mockResolvedValueOnce(page(["新剧", "下一部"], 3));
    const view = render(<CategoryBrowser api={mock} onSelect={vi.fn()} detectOrientation={false} />);
    fireEvent.click(await view.findByRole("button", { name: "加载更多" }));
    fireEvent.click(view.getByRole("button", { name: "现代" }));
    await view.findByRole("heading", { name: "新剧" });
    await act(async () => stale.resolve(page(["过期结果"])));
    expect(view.queryByText("过期结果")).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "加载更多" }));
    await view.findByRole("heading", { name: "下一部" });
    expect(view.container.querySelectorAll(".poster-card")).toHaveLength(2);
  });
  it("keeps loaded items after an error and retries the same page", async () => {
    const mock = api();
    mock.fetchWebCategory.mockResolvedValueOnce(page(["保留剧"], 2, true)).mockRejectedValueOnce(new Error("网络异常")).mockResolvedValueOnce(page(["下一部"], 3));
    const view = render(<CategoryBrowser api={mock} onSelect={vi.fn()} detectOrientation={false} />);
    fireEvent.click(await view.findByRole("button", { name: "加载更多" }));
    await view.findByRole("alert");
    expect(view.getByRole("heading", { name: "保留剧" })).toBeTruthy();
    expect(view.queryByText("没有符合条件的真人剧")).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "重试" }));
    await view.findByRole("heading", { name: "下一部" });
    expect(mock.fetchWebCategory).toHaveBeenLastCalledWith("drama", DEFAULT_CATEGORY_FILTERS, 2);
  });
});
