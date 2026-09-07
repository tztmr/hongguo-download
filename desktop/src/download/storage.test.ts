import { describe, expect, it } from "vitest";
import { createInitialState, enqueueEpisodes } from "./model";
import { DOWNLOAD_STORAGE_KEY, loadDownloadState, saveDownloadState } from "./storage";

function memoryStorage(seed: Record<string, string> = {}): Storage {
  const values = new Map(Object.entries(seed));
  return {
    get length() {
      return values.size;
    },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => [...values.keys()][index] ?? null,
    removeItem: (key) => values.delete(key),
    setItem: (key, value) => values.set(key, value),
  };
}

function memoryStorageWith(state: unknown): Storage {
  return memoryStorage({ [DOWNLOAD_STORAGE_KEY]: JSON.stringify(state) });
}

const v1Fixture = {
  version: 1,
  concurrency: 3,
  globallyPaused: true,
  batches: [
    {
      id: "batch-1",
      bookId: "book-a",
      title: "天下第一纨绔",
      cover: "cover-a",
      paused: true,
      items: [
        {
          id: "item-1",
          itemId: "e1",
          episodeIndex: 1,
          episodeTitle: "第 1 集",
          definition: "720p",
          status: "done",
          percent: 100,
          received: 12,
          total: 12,
          path: "/tmp/e1.mp4",
          addedAt: 1000,
          completedAt: 2000,
        },
      ],
      createdAt: 1000,
      updatedAt: 2000,
      completionNotifiedAt: 2100,
    },
  ],
};

function runningFixture() {
  const series = {
    bookId: "book-a",
    seriesId: "book-a",
    title: "天下第一纨绔",
    cover: "cover",
    firstVid: "",
    contentTypeCode: 1,
    episodeCount: 1,
    abstract: "",
    score: "",
    category: "真人剧",
    categoryTags: ["都市", "逆袭", "甜宠"],
    author: "",
    rankTags: [],
  };
  const queued = enqueueEpisodes(
    createInitialState(),
    series,
    [{ index: 1, itemId: "e1", title: "第 1 集" }],
    "auto",
    (() => {
      let value = 0;
      return () => `id-${++value}`;
    })(),
    1000,
  ).state;
  queued.batches[0].items[0] = {
    ...queued.batches[0].items[0],
    status: "running",
    percent: 72,
    received: 720,
    total: 1000,
  };
  return queued;
}

describe("download storage", () => {
  it("migrates v1 completed paths into v2 without resetting done items", () => {
    const storage = memoryStorageWith(v1Fixture);
    const restored = loadDownloadState(storage);

    expect(restored.state.version).toBe(2);
    expect(restored.state.batches[0].items[0]).toMatchObject({ status: "done", path: "/tmp/e1.mp4" });
    expect(restored.state.batches[0]).toMatchObject({
      paused: true,
      completionNotifiedAt: 2100,
      series: {
        bookId: "book-a",
        seriesId: "book-a",
        title: "天下第一纨绔",
        cover: "cover-a",
        abstract: "",
        category: "",
        contentTypeCode: 1,
      },
    });
    expect(restored.state.concurrency).toBe(3);
    expect(restored.state.globallyPaused).toBe(true);

    saveDownloadState(storage, restored.state);
    expect(JSON.parse(storage.getItem(DOWNLOAD_STORAGE_KEY)!).version).toBe(2);
  });

  it("restores running items as fresh queued items", () => {
    const storage = memoryStorage({ [DOWNLOAD_STORAGE_KEY]: JSON.stringify(runningFixture()) });
    const result = loadDownloadState(storage);
    const item = result.state.batches[0].items[0];

    expect(item).toMatchObject({ status: "queued", percent: 0, received: 0 });
    expect(result.state.batches[0].series.categoryTags).toEqual(["都市", "逆袭", "甜宠"]);
    expect(item.total).toBeUndefined();
    expect(result.warning).toBeUndefined();
  });

  it("preserves completed paths and clamps restored concurrency", () => {
    const fixture = runningFixture();
    fixture.concurrency = 99;
    fixture.batches[0].items[0] = {
      ...fixture.batches[0].items[0],
      status: "done",
      percent: 100,
      path: "/Downloads/ep1.mp4",
      completedAt: 2000,
    };
    const storage = memoryStorage({ [DOWNLOAD_STORAGE_KEY]: JSON.stringify(fixture) });

    const result = loadDownloadState(storage);

    expect(result.state.concurrency).toBe(10);
    expect(result.state.batches[0].items[0]).toMatchObject({ status: "done", path: "/Downloads/ep1.mp4" });
  });

  it("marks legacy fully completed batches as already notified", () => {
    const fixture = runningFixture();
    fixture.batches[0].items[0] = { ...fixture.batches[0].items[0], status: "done", percent: 100 };
    const legacy = JSON.parse(JSON.stringify(fixture));
    delete legacy.batches[0].completionNotifiedAt;
    const result = loadDownloadState(memoryStorage({ [DOWNLOAD_STORAGE_KEY]: JSON.stringify(legacy) }));
    expect(result.state.batches[0].completionNotifiedAt).toBe(fixture.batches[0].updatedAt);
  });

  it("backs up corrupt data and starts with an empty queue", () => {
    const storage = memoryStorage({ [DOWNLOAD_STORAGE_KEY]: "{not-json" });

    const result = loadDownloadState(storage);
    const backupKey = Array.from({ length: storage.length }, (_, index) => storage.key(index)).find((key) =>
      key?.startsWith("hongguo.downloads.corrupt."),
    );

    expect(result.state).toEqual(createInitialState());
    expect(result.warning).toContain("下载记录损坏");
    expect(backupKey).toBeTruthy();
    expect(storage.getItem(backupKey!)).toBe("{not-json");
  });

  it("persists the exact versioned state snapshot", () => {
    const storage = memoryStorage();
    const state = runningFixture();

    saveDownloadState(storage, state);

    expect(JSON.parse(storage.getItem(DOWNLOAD_STORAGE_KEY)!)).toEqual(state);
  });
});
