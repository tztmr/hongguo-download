import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { NewReleasePage, SeriesItem } from "../types";
import { saveReleaseItems } from "./storage";
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
});
