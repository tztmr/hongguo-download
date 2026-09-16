import { expect, it, vi } from "vitest";
import type { DiscoveryPage, SeriesItem } from "../types";
import { fillUniqueGroup, createGroupedPagingState } from "./groupedPaging";
import { fetchHomeDiscovery, fetchHomeDiscoveryMore } from "./homeDiscovery";

function item(id: string) { return { bookId: id } as SeriesItem; }
function recommendation(hasMore = false): DiscoveryPage {
  return { items: [item("recommended")], hasMore, nextOffset: 6, cellId: "cell", sessionId: "session", planId: "plan", filterIds: "", selectedItems: "", categories: [], rankBoards: [] };
}
function api() {
  return {
    fetchDiscovery: vi.fn().mockResolvedValue(recommendation()),
    fetchDiscoveryMore: vi.fn().mockResolvedValue(recommendation()),
    fetchWebCategory: vi.fn().mockImplementation(async (_type, _filters, page: number) => ({
      items: [item("recommended"), ...Array.from({ length: 24 }, (_, i) => item(`catalog-${(page - 1) * 24 + i}`))],
      hasMore: page < 10, nextPage: page + 1, total: 240,
    })),
  };
}

it.each(["drama", "manju"] as const)("continues %s beyond 100 unique home items after the recommendation session ends", async (type) => {
  const mock = api();
  let state = createGroupedPagingState<SeriesItem, Awaited<ReturnType<typeof fetchHomeDiscovery>> | null>(null);
  const counts = [];
  for (let click = 0; click < 14 && (state.hasMore || state.visibleCount < state.allItems.length); click++) {
    const result = await fillUniqueGroup(state, async cursor => {
      const page = cursor ? await fetchHomeDiscoveryMore(type, cursor, mock) : await fetchHomeDiscovery(type, mock);
      return { items: page.items, nextCursor: page, hasMore: page.hasMore };
    });
    state = result.state;
    counts.push(result.visible.length);
  }
  expect(counts).toContain(120);
  expect(state.allItems).toHaveLength(241);
  expect(state.visibleCount).toBe(241);
  expect(new Set(state.allItems.map(item => item.bookId)).size).toBe(241);
  expect(mock.fetchWebCategory.mock.calls.map(call => call[2])).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
  expect(mock.fetchDiscoveryMore).not.toHaveBeenCalled();
  expect(state.hasMore).toBe(false);
});

it("finishes genuine recommendation pages before continuing the library and retries a failed library page", async () => {
  const mock = api();
  mock.fetchDiscovery.mockResolvedValueOnce(recommendation(true));
  const first = await fetchHomeDiscovery("drama", mock);
  const next = await fetchHomeDiscoveryMore("drama", first, mock);
  expect(mock.fetchDiscoveryMore).toHaveBeenCalledOnce();
  expect(mock.fetchWebCategory).not.toHaveBeenCalled();
  mock.fetchWebCategory.mockRejectedValueOnce(new Error("断线"));
  await expect(fetchHomeDiscoveryMore("drama", next, mock)).rejects.toThrow("断线");
  const retried = await fetchHomeDiscoveryMore("drama", next, mock);
  expect(mock.fetchWebCategory.mock.calls.map(call => call[2])).toEqual([1, 1]);
  expect(retried.catalogNextPage).toBe(2);
});

it("continues a nine-item manju session through the matching video library", async () => {
  const mock = api();
  mock.fetchDiscovery.mockResolvedValueOnce({ ...recommendation(), items: Array.from({ length: 9 }, (_, i) => item(`manju-${i}`)) });
  const first = await fetchHomeDiscovery("manju", mock);
  expect(first.items).toHaveLength(9);
  expect(first.hasMore).toBe(true);
  const next = await fetchHomeDiscoveryMore("manju", first, mock);
  expect(next.items.length).toBeGreaterThan(9);
  expect(mock.fetchWebCategory).toHaveBeenCalledWith("manju", expect.objectContaining({ sort_type: "1" }), 1);
});
