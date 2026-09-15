import { expect, it, vi } from "vitest";
import { combinedDiscovery, discoveryCategoryGroups } from "./combinedDiscovery";
import type { DiscoveryPage, RankPage, SeriesItem } from "../types";
const page = (id: string, hasMore: boolean): DiscoveryPage => ({ items: [{ bookId: id } as SeriesItem], hasMore,
  cellId: id, nextOffset: 1, sessionId: id, planId: "", filterIds: "", selectedItems: "", categories: [], rankBoards: [] });
it("combines both types and advances only sources with remaining pages", async () => {
  const first = vi.fn(async (type) => page(type, type === "manju"));
  const more = vi.fn(async (type) => page(`${type}-next`, false));
  const initial = await combinedDiscovery(null, first, more);
  expect(initial.items.map((item) => item.bookId)).toEqual(["drama", "manju"]);
  expect(initial.hasMore).toBe(true);
  const next = await combinedDiscovery(initial, first, more);
  expect(next.items.map((item) => item.bookId)).toEqual(["manju-next"]);
  expect(more).toHaveBeenCalledExactlyOnceWith("manju", initial.sources!.manju);
  expect(next.hasMore).toBe(false);
});

it("mixes AI recommendations and retries only failed sources without advancing healthy cursors", async () => {
  const first = vi.fn(async type => { if (type === "manju") throw { message: "漫剧维护中" }; return page("drama", true); });
  const more = vi.fn(async () => page("drama-next", false));
  const ai = vi.fn(async () => ({ items: [page("ai", false).items[0], page("drama", false).items[0]], nextCursor: "ai-next", hasMore: true } as RankPage));
  const initial = await combinedDiscovery(null, first, more, ai);
  expect(initial.items.map(item => item.bookId)).toEqual(["drama", "ai"]);
  expect(initial.sourceErrors).toEqual({ manju: "漫剧维护中" });
  first.mockResolvedValueOnce(page("manju", false));
  const recovered = await combinedDiscovery(initial, first, more, ai, true);
  expect(recovered.items.map(item => item.bookId)).toEqual(["manju"]);
  expect(first.mock.calls.map(([type]) => type)).toEqual(["drama", "manju", "manju"]);
  expect(more).not.toHaveBeenCalled();
  expect(ai).toHaveBeenCalledTimes(1);
  expect(recovered.sources?.drama).toBe(initial.sources?.drama);
  expect(recovered.aiPage).toBe(initial.aiPage);
  expect(recovered.sourceErrors).toEqual({});
});

it("keeps failed page cursors and does not repeatedly hit a failed source while browsing", async () => {
  const first = vi.fn(async type => page(type, true));
  const more = vi.fn(async type => { if (type === "manju") throw new Error("暂时断开"); return page("drama-next", true); });
  const initial = await combinedDiscovery(null, first, more);
  const second = await combinedDiscovery(initial, first, more);
  const third = await combinedDiscovery(second, first, more);
  expect(third.sourceErrors?.manju).toBe("暂时断开");
  expect(more.mock.calls.filter(([type]) => type === "manju")).toHaveLength(1);
  more.mockResolvedValueOnce(page("manju-retry", false));
  await combinedDiscovery(third, first, more, undefined, true);
  expect(more).toHaveBeenLastCalledWith("manju", initial.sources?.manju);
});

it("groups the actual recommendation categories and deduplicates their IDs", () => {
  expect(discoveryCategoryGroups([{ id: "cate_20", name: "玄幻", group: "题材" }, { id: "cate_20", name: "玄幻", group: "题材" }]))
    .toEqual([{ id: "题材", name: "题材", items: [{ id: "all", name: "全部" }, { id: "cate_20", name: "玄幻" }] }]);
});
