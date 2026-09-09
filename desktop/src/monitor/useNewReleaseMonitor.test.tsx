import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { NewReleasePage, SeriesItem } from "../types";
import { loadReleaseItems, saveReleaseItems } from "./storage";
import { useNewReleaseMonitor } from "./useNewReleaseMonitor";

function storage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() { return values.size; }, clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null, key: (index) => [...values.keys()][index] ?? null,
    removeItem: (key) => values.delete(key), setItem: (key, value) => values.set(key, value),
  };
}

function release(id: string, category = "都市"): SeriesItem {
  return { bookId: id, seriesId: id, title: id, cover: "", firstVid: "", contentTypeCode: 1,
    episodeCount: 1, abstract: "", score: "", category, author: "", rankTags: [] };
}

function page(ids: string[], hasMore = false, cursor = "", category = "都市"): NewReleasePage {
  return { items: ids.map((id) => release(id, category)), hasMore, nextCursor: cursor, date: "2026-09-03", refreshedAt: "2026-09-03T09:00:00+08:00" };
}

async function flush() {
  await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); await Promise.resolve();
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-09-03T09:00:00+08:00"));
});
afterEach(() => vi.useRealTimers());

describe("useNewReleaseMonitor", () => {
  it("drains every cursor page without a fixed hop limit", async () => {
    const pages = Array.from({ length: 12 }, (_, index) =>
      page([`release-${index + 1}`], index < 11, index < 11 ? `cursor-${index + 1}` : ""));
    const api = { fetchNewReleases: vi.fn().mockImplementation(async () => pages.shift()!) };
    const seenStorage = storage();
    const notifications = { getStatus: vi.fn(), send: vi.fn() };
    const model = renderHook(() => useNewReleaseMonitor({
      api, storage: seenStorage, notifications, enabled: true, notify: false,
    }));

    await act(async () => { await flush(); });

    expect(api.fetchNewReleases).toHaveBeenCalledTimes(12);
    expect(model.result.current.items).toHaveLength(12);
    expect(model.result.current.items[model.result.current.items.length - 1]?.bookId).toBe("release-12");
    expect(model.result.current.hasMore).toBe(false);
  });

  it("uses a silent baseline then sends one aggregate notification for later IDs", async () => {
    vi.useFakeTimers();
    const api = { fetchNewReleases: vi.fn().mockResolvedValueOnce(page(["a"])).mockResolvedValueOnce(page(["a", "b", "c"])) };
    const notifications = { getStatus: vi.fn(), send: vi.fn().mockResolvedValue(true) };
    const seenStorage = storage();
    const result = renderHook(() => useNewReleaseMonitor({ api, storage: seenStorage, notifications, enabled: true, notify: true }));

    await act(async () => { await flush(); });
    expect(notifications.send).not.toHaveBeenCalled();
    await act(async () => { await vi.advanceTimersByTimeAsync(300_000); });

    expect(notifications.send).toHaveBeenCalledWith(expect.objectContaining({
      title: "发现今日新剧",
      body: expect.stringContaining("2 部"),
      target: { kind: "monitor", id: "playlet" },
    }));
    expect(result.result.current.unseenCount).toBe(2);
  });

  it("merges persisted same-day rows that have left the upstream window", async () => {
    const seenStorage = storage();
    saveReleaseItems(seenStorage, "2026-09-03", "playlet", [release("persisted", "古风")]);
    const api = { fetchNewReleases: vi.fn().mockResolvedValue(page(["latest"], false, "", "校园")) };
    const notifications = { getStatus: vi.fn(), send: vi.fn() };
    const model = renderHook(() => useNewReleaseMonitor({
      api, storage: seenStorage, notifications, enabled: true, notify: false,
    }));

    await act(async () => { await flush(); });

    expect(model.result.current.items.map((item) => item.bookId).sort()).toEqual(["latest", "persisted"]);
    expect(model.result.current.categories).toEqual(["古风", "校园"]);
    act(() => model.result.current.setCategory("校园"));
    expect(model.result.current.filteredItems.map((item) => item.bookId)).toEqual(["latest"]);
  });

  it("preserves existing rows when a refresh fails", async () => {
    const api = { fetchNewReleases: vi.fn().mockResolvedValueOnce(page(["kept"])).mockRejectedValueOnce(new Error("network down")) };
    const seenStorage = storage();
    const notifications = { getStatus: vi.fn(), send: vi.fn() };
    const model = renderHook(() => useNewReleaseMonitor({
      api, storage: seenStorage, notifications, enabled: true, notify: false,
    }));
    await act(async () => { await flush(); });

    await act(async () => { await model.result.current.refresh(); });

    expect(model.result.current.items.map((item) => item.bookId)).toEqual(["kept"]);
    expect(model.result.current.error).toBe("network down");
  });

  it("starts a request for the latest type after an older request finishes", async () => {
    let resolvePlaylet!: (value: NewReleasePage) => void;
    const pending = new Promise<NewReleasePage>((resolve) => { resolvePlaylet = resolve; });
    const api = { fetchNewReleases: vi.fn().mockReturnValueOnce(pending).mockResolvedValueOnce(page(["ai-release"])) };
    const seenStorage = storage();
    const notifications = { getStatus: vi.fn(), send: vi.fn() };
    const model = renderHook(() => useNewReleaseMonitor({
      api, storage: seenStorage, notifications, enabled: true, notify: false,
    }));

    await act(async () => { model.result.current.setType("ai_playlet"); });
    await act(async () => { resolvePlaylet(page(["playlet-release"])); await flush(); });

    expect(api.fetchNewReleases).toHaveBeenNthCalledWith(2, "ai_playlet", "", 20);
    expect(model.result.current.items.map((item) => item.bookId)).toEqual(["ai-release"]);
  });

  it("shows each page immediately and retains it when a later page fails", async () => {
    const second = deferred<NewReleasePage>();
    const api = { fetchNewReleases: vi.fn().mockResolvedValueOnce(page(["visible"], true, "next")).mockReturnValueOnce(second.promise) };
    const seenStorage = storage();
    const notifications = { getStatus: vi.fn(), send: vi.fn() };
    const model = renderHook(() => useNewReleaseMonitor({ api, storage: seenStorage, notifications, enabled: true, notify: true }));
    await act(flush);
    expect(model.result.current.items.map((item) => item.bookId)).toEqual(["visible"]);
    expect(model.result.current.loading).toBe(true);
    expect(model.result.current.scanPages).toBe(1);
    expect(model.result.current.scanComplete).toBe(false);
    await act(async () => { second.reject(new Error("第二页不可用")); await flush(); });
    expect(model.result.current.error).toBe("第二页不可用");
    expect(model.result.current.items).toHaveLength(1);
    expect(model.result.current.scanComplete).toBe(false);
    expect(model.result.current.loading).toBe(false);
    expect(loadReleaseItems(seenStorage, "2026-09-03", "playlet")).toHaveLength(1);
    expect(notifications.send).not.toHaveBeenCalled();
  });

  it("rejects repeated cursors without discarding fetched rows", async () => {
    const api = { fetchNewReleases: vi.fn().mockResolvedValueOnce(page(["a"], true, "same")).mockResolvedValueOnce(page(["b"], true, "same")) };
    const seenStorage = storage();
    const notifications = { getStatus: vi.fn(), send: vi.fn() };
    const model = renderHook(() => useNewReleaseMonitor({ api, storage: seenStorage, notifications, enabled: true, notify: false }));
    await act(flush);
    expect(api.fetchNewReleases).toHaveBeenCalledTimes(2);
    expect(model.result.current.items).toHaveLength(2);
    expect(model.result.current.error).toContain("分页状态未前进");
    expect(model.result.current.scanComplete).toBe(false);
  });

  it("does not continue or publish an old scan after a quick A to B to A switch", async () => {
    const old = deferred<NewReleasePage>();
    const api = { fetchNewReleases: vi.fn().mockReturnValueOnce(old.promise).mockResolvedValueOnce(page(["current"])) };
    const seenStorage = storage();
    const notifications = { getStatus: vi.fn(), send: vi.fn() };
    const model = renderHook(() => useNewReleaseMonitor({ api, storage: seenStorage, notifications, enabled: true, notify: false }));
    await act(async () => { model.result.current.setType("ai_playlet"); model.result.current.setType("playlet"); await flush(); });
    await act(async () => { old.resolve(page(["stale"], true, "stale-next")); await flush(); });
    expect(api.fetchNewReleases).toHaveBeenCalledTimes(2);
    expect(model.result.current.items.map((item) => item.bookId)).toEqual(["current"]);
    expect(model.result.current.scanComplete).toBe(true);
  });

  it.each(["disable", "unmount"])("stops old pagination and notifications on %s", async (action) => {
    const pending = deferred<NewReleasePage>();
    const api = { fetchNewReleases: vi.fn().mockReturnValueOnce(pending.promise) };
    const seenStorage = storage();
    const notifications = { getStatus: vi.fn(), send: vi.fn() };
    const model = renderHook(({ enabled }) => useNewReleaseMonitor({ api, storage: seenStorage, notifications, enabled, notify: true }), { initialProps: { enabled: true } });
    if (action === "disable") model.rerender({ enabled: false });
    else model.unmount();
    await act(async () => { pending.resolve(page(["stale"], true, "next")); await flush(); });
    expect(api.fetchNewReleases).toHaveBeenCalledTimes(1);
    expect(loadReleaseItems(seenStorage, "2026-09-03", "playlet")).toEqual([]);
    expect(notifications.send).not.toHaveBeenCalled();
  });

  it("rejects a changed page date instead of merging different days", async () => {
    const api = { fetchNewReleases: vi.fn().mockResolvedValueOnce(page(["today"], true, "next")).mockResolvedValueOnce({ ...page(["tomorrow"]), date: "2026-09-04" }) };
    const seenStorage = storage();
    const notifications = { getStatus: vi.fn(), send: vi.fn() };
    const model = renderHook(() => useNewReleaseMonitor({ api, storage: seenStorage, notifications, enabled: true, notify: false }));
    await act(flush);
    expect(model.result.current.items.map((item) => item.bookId)).toEqual(["today"]);
    expect(model.result.current.error).toContain("分页日期发生变化");
    expect(model.result.current.scanComplete).toBe(false);
  });

  it("clears yesterday's rows immediately when a midnight refresh starts", async () => {
    const today = deferred<NewReleasePage>();
    const api = { fetchNewReleases: vi.fn().mockResolvedValueOnce(page(["yesterday"])).mockReturnValueOnce(today.promise) };
    const seenStorage = storage();
    const notifications = { getStatus: vi.fn(), send: vi.fn() };
    const model = renderHook(() => useNewReleaseMonitor({ api, storage: seenStorage, notifications, enabled: true, notify: false }));
    await act(flush);
    vi.setSystemTime(new Date("2026-09-04T00:01:00+08:00"));
    act(() => { void model.result.current.refresh(); });
    expect(model.result.current.items).toEqual([]);
    expect(model.result.current.date).toBe("2026-09-04");
    await act(async () => { today.resolve({ ...page(["today"]), date: "2026-09-04" }); await flush(); });
    expect(model.result.current.items.map((item) => item.bookId)).toEqual(["today"]);
  });

  it("does not publish a response that arrives across midnight", async () => {
    const pending = deferred<NewReleasePage>();
    const api = { fetchNewReleases: vi.fn().mockReturnValueOnce(pending.promise) };
    const seenStorage = storage();
    const notifications = { getStatus: vi.fn(), send: vi.fn() };
    const model = renderHook(() => useNewReleaseMonitor({ api, storage: seenStorage, notifications, enabled: true, notify: false }));
    vi.setSystemTime(new Date("2026-09-04T00:01:00+08:00"));
    await act(async () => { pending.resolve(page(["yesterday"], true, "next")); await flush(); });
    expect(model.result.current.items).toEqual([]);
    expect(model.result.current.date).toBe("2026-09-04");
    expect(model.result.current.error).toContain("已跨日");
    expect(api.fetchNewReleases).toHaveBeenCalledTimes(1);
  });

  it("uses latest-rank notification wording for a latest feed", async () => {
    const api = { fetchNewReleases: vi.fn().mockResolvedValueOnce(page([])).mockResolvedValueOnce({ ...page(["a"]), dateScope: "latest", source: "rank" }).mockResolvedValueOnce({ ...page(["a", "b"]), dateScope: "latest", source: "rank" }) };
    const seenStorage = storage();
    const notifications = { getStatus: vi.fn(), send: vi.fn().mockResolvedValue(true) };
    const model = renderHook(() => useNewReleaseMonitor({ api, storage: seenStorage, notifications, enabled: true, notify: true }));
    await act(flush);
    await act(async () => { model.result.current.setType("ai_playlet"); await flush(); });
    await act(async () => { await model.result.current.refresh(); });
    expect(notifications.send).toHaveBeenCalledWith(expect.objectContaining({ title: "发现新收录剧目", body: "新剧榜新增收录 1 部剧目" }));
    expect(model.result.current.source).toBe("rank");
    expect(model.result.current.dateScope).toBe("latest");
  });

  it("filters titles and original tags, and sorts known metrics ahead of missing values", async () => {
    const rows = [
      { ...release("a", "仙侠"), title: "仙侠归来", hotCount: 10, collectCount: 40, onlineTime: 5 },
      { ...release("b", "仙侠"), title: "仙侠传说", hotCount: 30, collectCount: 20, onlineTime: 10 },
      { ...release("c", "武侠"), title: "侠客归来", onlineTime: 20 },
    ];
    const api = { fetchNewReleases: vi.fn().mockResolvedValue({ ...page([]), items: rows }) };
    const seenStorage = storage();
    const notifications = { getStatus: vi.fn(), send: vi.fn() };
    const model = renderHook(() => useNewReleaseMonitor({ api, storage: seenStorage, notifications, enabled: true, notify: false }));
    await act(flush);
    act(() => model.result.current.setSort("hot"));
    expect(model.result.current.filteredItems.map((item) => item.bookId)).toEqual(["b", "a", "c"]);
    act(() => { model.result.current.setCategory("仙侠"); model.result.current.setSort("collected"); });
    expect(model.result.current.filteredItems.map((item) => item.bookId)).toEqual(["a", "b"]);
    act(() => model.result.current.setQuery("  归来 "));
    expect(model.result.current.filteredItems.map((item) => item.bookId)).toEqual(["a"]);
  });
});
