import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { EpisodeItem, SeriesItem } from "../types";
import type { DownloadAdapter, DownloadProgress } from "./useDownloadManager";
import { useDownloadManager } from "./useDownloadManager";

function memoryStorage(): Storage {
  const values = new Map<string, string>();
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

function deferredAdapter() {
  const started: string[] = [];
  const controls = new Map<
    string,
    {
      resolve: (value: { taskId: string; path: string; definition: string; bytes: number }) => void;
      reject: (reason: Error) => void;
    }
  >();
  let listener: ((progress: DownloadProgress) => void) | undefined;
  const adapter: DownloadAdapter = {
    download: (args) =>
      new Promise((resolve, reject) => {
        started.push(args.taskId);
        controls.set(args.taskId, { resolve, reject });
      }),
    subscribeProgress: async (nextListener) => {
      listener = nextListener;
      return () => {
        listener = undefined;
      };
    },
  };
  return {
    adapter,
    started,
    resolve(taskId: string) {
      controls.get(taskId)?.resolve({ taskId, path: `/Downloads/${taskId}.mp4`, definition: "1080p", bytes: 100 });
      controls.delete(taskId);
    },
    reject(taskId: string) {
      controls.get(taskId)?.reject(new Error("network down"));
      controls.delete(taskId);
    },
    progress(progress: DownloadProgress) {
      listener?.(progress);
    },
    resolveAll() {
      [...controls.keys()].forEach((taskId) => this.resolve(taskId));
    },
  };
}

const series: SeriesItem = {
  bookId: "book-a",
  seriesId: "book-a",
  title: "天下第一纨绔",
  cover: "cover",
  firstVid: "",
  contentTypeCode: 1,
  episodeCount: 6,
  abstract: "",
  score: "",
  category: "真人剧",
  author: "",
  rankTags: [],
};

const episodes: EpisodeItem[] = Array.from({ length: 6 }, (_, offset) => ({
  index: offset + 1,
  itemId: `episode-${offset + 1}`,
  title: `第 ${offset + 1} 集`,
}));

const manyEpisodes: EpisodeItem[] = Array.from({ length: 10 }, (_, offset) => ({
  index: offset + 1,
  itemId: `many-episode-${offset + 1}`,
  title: `第 ${offset + 1} 集`,
}));

describe("useDownloadManager", () => {
  it("starts eight downloads by default and fills a freed slot", async () => {
    const fake = deferredAdapter();
    const { result, unmount } = renderHook(() => useDownloadManager({ adapter: fake.adapter, storage: memoryStorage() }));

    act(() => result.current.enqueue(series, manyEpisodes));
    await waitFor(() => expect(fake.started).toHaveLength(8));
    act(() => fake.resolve(fake.started[0]));
    await waitFor(() => expect(fake.started).toHaveLength(9));

    act(() => fake.resolveAll());
    await waitFor(() => expect(fake.started).toHaveLength(10));
    act(() => fake.resolveAll());
    await waitFor(() => expect(result.current.stats.done).toBe(10));
    unmount();
  });

  it("does not fill freed slots after a global pause", async () => {
    const fake = deferredAdapter();
    const { result, unmount } = renderHook(() => useDownloadManager({ adapter: fake.adapter, storage: memoryStorage() }));

    act(() => result.current.enqueue(series, manyEpisodes));
    await waitFor(() => expect(fake.started).toHaveLength(8));
    act(() => result.current.pauseAll());
    act(() => fake.resolve(fake.started[0]));

    await waitFor(() => expect(result.current.stats.done).toBe(1));
    expect(fake.started).toHaveLength(8);
    act(() => fake.resolveAll());
    unmount();
  });

  it("keeps active downloads when concurrency is lowered and obeys the new limit afterward", async () => {
    const fake = deferredAdapter();
    const { result, unmount } = renderHook(() => useDownloadManager({ adapter: fake.adapter, storage: memoryStorage() }));

    act(() => result.current.enqueue(series, manyEpisodes));
    await waitFor(() => expect(fake.started).toHaveLength(8));
    act(() => result.current.setConcurrency(1));
    act(() => fake.resolve(fake.started[0]));
    await waitFor(() => expect(result.current.stats.done).toBe(1));
    expect(fake.started).toHaveLength(8);

    act(() => fake.resolveAll());
    unmount();
  });

  it("isolates a rejected episode and keeps its sibling running", async () => {
    const fake = deferredAdapter();
    const { result, unmount } = renderHook(() => useDownloadManager({ adapter: fake.adapter, storage: memoryStorage() }));

    act(() => result.current.enqueue(series, episodes.slice(0, 2)));
    await waitFor(() => expect(fake.started).toHaveLength(2));
    act(() => fake.reject(fake.started[0]));

    await waitFor(() => expect(result.current.stats.error).toBe(1));
    expect(result.current.stats.running).toBe(1);
    expect(result.current.state.batches[0].items[0].error).toContain("network down");

    act(() => fake.resolveAll());
    unmount();
  });

  it("automatically retries a failed episode three times before keeping it failed", async () => {
    vi.useFakeTimers();
    try {
      const fake = deferredAdapter();
      const storage = memoryStorage();
      const { result, unmount } = renderHook(() => useDownloadManager({ adapter: fake.adapter, storage }));

      act(() => result.current.enqueue(series, episodes.slice(0, 1)));
      await act(async () => {
        await Promise.resolve();
      });
      expect(fake.started).toHaveLength(1);

      for (const expectedAttempts of [2, 3, 4]) {
        act(() => fake.reject(fake.started[fake.started.length - 1]!));
        await act(async () => {
          await Promise.resolve();
          await Promise.resolve();
        });
        expect(result.current.stats.error).toBe(1);
        await act(async () => {
          await vi.advanceTimersByTimeAsync(10_000);
          await Promise.resolve();
          await Promise.resolve();
        });
        if (expectedAttempts < 4) {
          expect(fake.started).toHaveLength(expectedAttempts);
        }
      }

      act(() => fake.reject(fake.started[fake.started.length - 1]!));
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(fake.started).toHaveLength(4);
      expect(result.current.state.batches[0].items[0]).toMatchObject({ status: "error", error: "network down" });
      unmount();
    } finally {
      vi.useRealTimers();
    }
  });

  it("maps progress events to the matching running episode", async () => {
    const fake = deferredAdapter();
    const { result, unmount } = renderHook(() => useDownloadManager({ adapter: fake.adapter, storage: memoryStorage() }));

    act(() => result.current.enqueue(series, episodes.slice(0, 1)));
    await waitFor(() => expect(fake.started).toHaveLength(1));
    act(() => fake.progress({ taskId: fake.started[0], received: 50, total: 100, percent: 50 }));

    await waitFor(() => expect(result.current.state.batches[0].items[0].percent).toBe(50));
    expect(result.current.state.batches[0].items[0].received).toBe(50);

    act(() => fake.resolveAll());
    unmount();
  });

  it("emits one callback when a batch transitions to fully completed", async () => {
    const fake = deferredAdapter();
    const onBatchCompleted = vi.fn();
    const sharedStorage = memoryStorage();
    const { result, rerender, unmount } = renderHook(() => useDownloadManager({
      adapter: fake.adapter,
      storage: sharedStorage,
      onBatchCompleted,
    }));
    act(() => result.current.enqueue(series, episodes.slice(0, 2)));
    await waitFor(() => expect(fake.started).toHaveLength(2));
    act(() => fake.resolve(fake.started[0]));
    await waitFor(() => expect(result.current.stats.done).toBe(1));
    expect(onBatchCompleted).not.toHaveBeenCalled();
    act(() => fake.resolveAll());
    await waitFor(() => expect(onBatchCompleted).toHaveBeenCalledTimes(1));
    expect(onBatchCompleted.mock.calls[0][0]).toMatchObject({ title: series.title });
    rerender();
    await act(async () => Promise.resolve());
    expect(onBatchCompleted).toHaveBeenCalledTimes(1);
    expect(result.current.state.batches[0].completionNotifiedAt).toEqual(expect.any(Number));
    unmount();
  });
});
