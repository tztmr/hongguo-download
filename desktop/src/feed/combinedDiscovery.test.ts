import { expect, it, vi } from "vitest";
import { combinedDiscovery } from "./combinedDiscovery";
import type { DiscoveryPage, SeriesItem } from "../types";
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
