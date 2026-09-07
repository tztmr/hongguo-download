import { describe, expect, it } from "vitest";
import { getDownloadStats } from "./download/model";
import { createPreviewDownloadState, previewEpisodes, previewSeries } from "./preview";

describe("preview fixtures", () => {
  it("provides the approved download-center statistics and grouped rows", () => {
    const state = createPreviewDownloadState();

    expect(state.version).toBe(2);
    expect(state.batches[0].series).toEqual({
      bookId: previewSeries[0].bookId,
      seriesId: previewSeries[0].seriesId,
      title: previewSeries[0].title,
      cover: previewSeries[0].cover,
      abstract: previewSeries[0].abstract,
      category: previewSeries[0].category,
      categoryTags: previewSeries[0].categoryTags,
      contentTypeCode: previewSeries[0].contentTypeCode,
    });
    expect(getDownloadStats(state)).toEqual({ running: 5, queued: 16, done: 150, error: 2 });
    expect(state.concurrency).toBe(5);
    expect(state.batches).toHaveLength(4);
    expect(state.batches[0].items).toHaveLength(59);
  });

  it("provides a complete library selection surface", () => {
    expect(previewSeries).toHaveLength(40);
    expect(previewSeries.every((series) => series.cover === "")).toBe(true);
    expect(previewSeries[0]).toMatchObject({ playCount: 0, onlineTime: expect.any(Number) });
    expect(previewEpisodes).toHaveLength(59);
  });
});
