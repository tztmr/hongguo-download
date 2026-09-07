import { describe, expect, it, vi } from "vitest";
import { SearchCache, searchKey } from "./cache";

describe("search cache", () => {
  it("shares pending requests, reuses successful results and expires them", async () => {
    let now = 0;
    const cache = new SearchCache<number>(2, 100, () => now);
    let resolve!: (value: number) => void;
    const load = vi.fn(() => new Promise<number>((done) => { resolve = done; }));
    const first = cache.getOrLoad("a", load);
    const second = cache.getOrLoad("a", load);
    await Promise.resolve();
    expect(load).toHaveBeenCalledTimes(1);
    resolve(42);
    expect(await first).toBe(42);
    expect(await second).toBe(42);
    expect(await cache.getOrLoad("a", load)).toBe(42);
    now = 101;
    expect(await cache.getOrLoad("a", async () => 99)).toBe(99);
  });
  it("evicts the least recently used entry and never caches errors", async () => {
    const cache = new SearchCache<number>(2);
    cache.set("a", 1); cache.set("b", 2);
    await cache.getOrLoad("a", async () => 9);
    cache.set("c", 3);
    expect(await cache.getOrLoad("b", async () => 4)).toBe(4);
    await expect(cache.getOrLoad("bad", async () => { throw Error("offline"); })).rejects.toThrow("offline");
    expect(await cache.getOrLoad("bad", async () => 8)).toBe(8);
  });
  it("refresh discards old in-flight results and protects the replacement request", async () => {
    const cache = new SearchCache<number>();
    let resolve!: (value: number) => void;
    const old = cache.getOrLoad("same", () => new Promise<number>((done) => { resolve = done; }));
    await Promise.resolve();
    cache.clear();
    expect(await cache.getOrLoad("same", async () => 2)).toBe(2);
    resolve(1); await old;
    expect(await cache.getOrLoad("same", async () => 3)).toBe(2);
    expect(searchKey(" 剧名 ", "all", "exact")).not.toBe(searchKey("剧名", "drama", "exact"));
    expect(searchKey("剧名", "all", "exact")).not.toBe(searchKey("剧名", "all", "fuzzy"));
  });
  it("does not retain oversized result sets", async () => {
    const cache = new SearchCache<number[]>(2, 100, Date.now, (items) => items.length <= 2);
    cache.set("large", [1, 2, 3]);
    expect(await cache.getOrLoad("large", async () => [4])).toEqual([4]);
  });
});
