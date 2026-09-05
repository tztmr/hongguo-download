import { describe, expect, it } from "vitest";
import type { SeriesItem } from "../types";
import {
  loadReleaseItems,
  loadSeenReleases,
  markSeenReleases,
  NEW_RELEASES_ITEMS_KEY,
  NEW_RELEASES_SEEN_KEY,
  saveReleaseItems,
} from "./storage";

function memoryStorage(seed?: string): Storage {
  const values = new Map<string, string>();
  if (seed !== undefined) values.set(NEW_RELEASES_SEEN_KEY, seed);
  return {
    get length() { return values.size; },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => [...values.keys()][index] ?? null,
    removeItem: (key) => values.delete(key),
    setItem: (key, value) => values.set(key, value),
  };
}

describe("new release seen storage", () => {
  it("falls back safely for corrupted data", () => {
    expect(loadSeenReleases(memoryStorage("{"), "2026-09-02", "playlet")).toEqual(new Set());
  });

  it("scopes IDs by Shanghai date and release type", () => {
    const storage = memoryStorage();
    markSeenReleases(storage, "2026-09-02", "playlet", ["a", "b"]);
    expect([...loadSeenReleases(storage, "2026-09-02", "playlet")]).toEqual(["a", "b"]);
    expect(loadSeenReleases(storage, "2026-09-02", "ai_playlet").size).toBe(0);
    expect(loadSeenReleases(storage, "2026-09-03", "playlet").size).toBe(0);
  });

  it("prunes entries from older dates on write", () => {
    const storage = memoryStorage(JSON.stringify({ version: 1, entries: {
      "2026-09-01|playlet": ["old"],
      "2026-09-02|playlet": ["today"],
    } }));
    markSeenReleases(storage, "2026-09-02", "playlet", ["new"]);
    const raw = JSON.parse(storage.getItem(NEW_RELEASES_SEEN_KEY)!);
    expect(raw.entries).toEqual({ "2026-09-02|playlet": ["today", "new"] });
  });

  it("persists full result snapshots by Shanghai date and type", () => {
    const storage = memoryStorage();
    const item: SeriesItem = {
      bookId: "persisted", seriesId: "persisted", title: "已收录", cover: "", firstVid: "",
      contentTypeCode: 1004, episodeCount: 8, abstract: "", score: "", category: "古风",
      categoryTags: ["古风"], releaseType: "ai_playlet", author: "", rankTags: [],
    };

    saveReleaseItems(storage, "2026-09-03", "ai_playlet", [item]);

    expect(loadReleaseItems(storage, "2026-09-03", "ai_playlet")).toEqual([item]);
    expect(loadReleaseItems(storage, "2026-09-03", "comic_series_rank")).toEqual([]);
    expect(loadReleaseItems(storage, "2026-09-02", "ai_playlet")).toEqual([]);
  });

  it("prunes old item snapshots when writing a new date", () => {
    const storage = memoryStorage();
    storage.setItem(NEW_RELEASES_ITEMS_KEY, JSON.stringify({
      version: 1,
      entries: {
        "2026-09-02|playlet": [{ bookId: "old", seriesId: "old", title: "旧剧" }],
      },
    }));

    saveReleaseItems(storage, "2026-09-03", "playlet", []);

    const raw = JSON.parse(storage.getItem(NEW_RELEASES_ITEMS_KEY)!);
    expect(raw.entries).toEqual({ "2026-09-03|playlet": [] });
  });
});
