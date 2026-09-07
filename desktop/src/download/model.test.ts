import { describe, expect, it } from "vitest";
import type { EpisodeItem, SeriesItem } from "../types";
import {
  createInitialState,
  deriveBatchStatus,
  downloadReducer,
  enqueueEpisodes,
  getDownloadStats,
  selectLaunchableItems,
} from "./model";

const seriesA: SeriesItem = {
  bookId: "book-a",
  seriesId: "book-a",
  title: "天下第一纨绔",
  cover: "cover-a",
  firstVid: "",
  contentTypeCode: 1,
  episodeCount: 4,
  durationSeconds: 3723,
  abstract: "",
  score: "",
  category: "真人剧",
  categoryTags: ["都市", "逆袭", "甜宠"],
  author: "",
  onlineTime: 1_787_760_360,
  rankTags: [],
};

const seriesB: SeriesItem = { ...seriesA, bookId: "book-b", seriesId: "book-b", title: "女子爱财，取之有道" };

const episodes: EpisodeItem[] = [1, 2, 3, 4].map((index) => ({
  index,
  itemId: `e${index}`,
  title: `第 ${index} 集`,
}));

function ids(prefix: string) {
  let value = 0;
  return () => `${prefix}-${++value}`;
}

function fixedIds() {
  let value = 0;
  return () => `fixed-${++value}`;
}

describe("download model", () => {
  it("captures immutable series metadata for later upload defaults", () => {
    const result = enqueueEpisodes(createInitialState(), seriesA, [episodes[0]], "auto", fixedIds(), 1000);

    expect(result.state.batches[0].series).toEqual({
      bookId: seriesA.bookId,
      seriesId: seriesA.seriesId,
      title: seriesA.title,
      cover: seriesA.cover,
      abstract: seriesA.abstract,
      category: seriesA.category,
      categoryTags: seriesA.categoryTags,
      contentTypeCode: seriesA.contentTypeCode,
      episodeCount: seriesA.episodeCount,
      durationSeconds: seriesA.durationSeconds,
      author: seriesA.author,
      onlineTime: seriesA.onlineTime,
      releaseType: seriesA.releaseType,
    });
  });

  it("snapshots the selected definition into every newly queued item", () => {
    const result = enqueueEpisodes(
      createInitialState(),
      seriesA,
      episodes.slice(0, 2),
      "720p",
      ids("definition"),
      1000,
    );

    expect(result.state.batches[0].items.map((item) => item.definition)).toEqual([
      "720p",
      "720p",
    ]);
  });

  it("groups episodes by series and skips episodes already recorded", () => {
    const initial = createInitialState();
    const first = enqueueEpisodes(initial, seriesA, episodes.slice(0, 2), "auto", ids("first"), 1000);
    const second = enqueueEpisodes(first.state, seriesA, episodes.slice(1, 3), "auto", ids("second"), 2000);

    expect(initial.concurrency).toBe(8);
    expect(second.state.batches).toHaveLength(1);
    expect(second.state.batches[0].items.map((item) => item.itemId)).toEqual(["e1", "e2", "e3"]);
    expect(second.added).toBe(1);
    expect(second.skipped).toBe(1);
  });

  it("clamps concurrency changes to the supported one through ten range", () => {
    const initial = createInitialState();
    const tooLow = downloadReducer(initial, { type: "set-concurrency", value: 0 });
    const tooHigh = downloadReducer(tooLow, { type: "set-concurrency", value: 50 });

    expect(tooLow.concurrency).toBe(1);
    expect(tooHigh.concurrency).toBe(10);
  });

  it("selects queued episodes fairly across runnable series", () => {
    const a = enqueueEpisodes(createInitialState(), seriesA, episodes, "auto", ids("a"), 1000).state;
    const b = enqueueEpisodes(a, seriesB, episodes.map((item) => ({ ...item, itemId: `b-${item.itemId}` })), "auto", ids("b"), 2000).state;

    const selected = selectLaunchableItems(b, new Set(), 6, 0);

    expect(selected.items.map(({ batchId }) => batchId)).toEqual([
      b.batches[0].id,
      b.batches[1].id,
      b.batches[0].id,
      b.batches[1].id,
      b.batches[0].id,
      b.batches[1].id,
    ]);
  });

  it("does not launch items while globally paused or while their series is paused", () => {
    const queued = enqueueEpisodes(createInitialState(), seriesA, episodes, "auto", ids("pause"), 1000).state;
    const globallyPaused = downloadReducer(queued, { type: "pause-all" });
    const batchPaused = downloadReducer(queued, { type: "pause-batch", batchId: queued.batches[0].id });

    expect(selectLaunchableItems(globallyPaused, new Set(), 5, 0).items).toEqual([]);
    expect(selectLaunchableItems(batchPaused, new Set(), 5, 0).items).toEqual([]);
  });

  it("updates progress, completion, failure and derived statistics without blocking siblings", () => {
    const queued = enqueueEpisodes(createInitialState(), seriesA, episodes.slice(0, 3), "auto", ids("status"), 1000).state;
    const [first, second] = queued.batches[0].items;
    const running = downloadReducer(queued, { type: "mark-running", itemId: first.id });
    const progressed = downloadReducer(running, {
      type: "update-progress",
      itemId: first.id,
      received: 50,
      total: 100,
      percent: 50,
    });
    const completed = downloadReducer(progressed, {
      type: "mark-done",
      itemId: first.id,
      path: "/Downloads/ep1.mp4",
      definition: "1080p",
      bytes: 100,
      completedAt: 3000,
    });
    const failed = downloadReducer(completed, { type: "mark-error", itemId: second.id, error: "network" });

    expect(failed.batches[0].items[0]).toMatchObject({ status: "done", percent: 100, path: "/Downloads/ep1.mp4" });
    expect(failed.batches[0].items[1]).toMatchObject({ status: "error", error: "network" });
    expect(failed.batches[0].items[2].status).toBe("queued");
    expect(getDownloadStats(failed)).toEqual({ running: 0, queued: 1, done: 1, error: 1 });
    expect(deriveBatchStatus(failed.batches[0])).toBe("queued");
  });

  it("retries failed items and removes completed-only batches without touching files", () => {
    const queued = enqueueEpisodes(createInitialState(), seriesA, episodes.slice(0, 2), "auto", ids("retry"), 1000).state;
    const [first, second] = queued.batches[0].items;
    const failed = downloadReducer(queued, { type: "mark-error", itemId: first.id, error: "timeout" });
    const retried = downloadReducer(failed, { type: "retry-item", itemId: first.id });
    const doneFirst = downloadReducer(retried, {
      type: "mark-done",
      itemId: first.id,
      path: "/Downloads/ep1.mp4",
      definition: "1080p",
      bytes: 10,
      completedAt: 2000,
    });
    const doneBoth = downloadReducer(doneFirst, {
      type: "mark-done",
      itemId: second.id,
      path: "/Downloads/ep2.mp4",
      definition: "1080p",
      bytes: 20,
      completedAt: 3000,
    });
    const cleared = downloadReducer(doneBoth, { type: "clear-completed" });

    expect(retried.batches[0].items[0]).toMatchObject({ status: "queued", percent: 0, error: undefined });
    expect(cleared.batches).toEqual([]);
  });

  it("defers removal of a running batch until its active item settles", () => {
    const queued = enqueueEpisodes(createInitialState(), seriesA, episodes.slice(0, 2), "auto", ids("remove"), 1000).state;
    const first = queued.batches[0].items[0];
    const running = downloadReducer(queued, { type: "mark-running", itemId: first.id });
    const pendingRemoval = downloadReducer(running, { type: "remove-batch", batchId: queued.batches[0].id });

    expect(pendingRemoval.batches[0].removeWhenIdle).toBe(true);
    expect(selectLaunchableItems(pendingRemoval, new Set([first.id]), 5, 0).items).toEqual([]);

    const settled = downloadReducer(pendingRemoval, {
      type: "mark-done",
      itemId: first.id,
      path: "/Downloads/ep1.mp4",
      definition: "1080p",
      bytes: 10,
      completedAt: 2000,
    });
    expect(settled.batches).toEqual([]);
  });

  it("marks a completed batch as notified exactly once", () => {
    const queued = enqueueEpisodes(createInitialState(), seriesA, episodes.slice(0, 1), "auto", ids("notify"), 1000).state;
    const batchId = queued.batches[0].id;
    const marked = downloadReducer(queued, { type: "mark-batch-notified", batchId, notifiedAt: 2000 });
    const repeated = downloadReducer(marked, { type: "mark-batch-notified", batchId, notifiedAt: 3000 });
    expect(repeated.batches[0].completionNotifiedAt).toBe(2000);
  });
});
