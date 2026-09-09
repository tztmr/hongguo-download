import { describe, expect, it } from "vitest";
import type { MediaJob } from "../media/types";
import { getUploadSourceOptions } from "./uploadSources";

const merge: MediaJob = { id: "merge", dedupeKey: "merge", kind: "merge", status: "completed", stage: "completed", percent: 100,
  inputs: [], outputPath: "C:\\剧集\\合并.mp4", errorCode: null, errorMessage: null };
const clean: MediaJob = { ...merge, id: "clean", kind: "separateBackgroundMusic", aiRequest: { title: "剧集", scope: "merged", model: "htdemucs" },
  inputs: [{ path: "C:/剧集/合并.mp4", sizeBytes: 100 }], outputPath: null,
  outputs: [{ episodeIndex: 1, kind: "noBackgroundMusicVideo", path: "C:\\剧集\\去背景音乐.mp4" }] };

describe("upload source versions", () => {
  it("offers the same original and processed pair from either entry, including Windows paths", () => {
    const expected = [{ kind: "merged", path: merge.outputPath }, { kind: "noBackgroundMusic", path: clean.outputs![0].path }];
    expect(getUploadSourceOptions(merge.outputPath!, [merge, clean])).toEqual(expected);
    expect(getUploadSourceOptions(clean.outputs![0].path, [merge, clean])).toEqual(expected);
  });

  it("does not offer another merge's result, unfinished work, or episode separation", () => {
    for (const other of [
      { ...clean, inputs: [{ path: "C:/其他剧/合并.mp4", sizeBytes: 100 }] },
      { ...clean, status: "running" as const },
      { ...clean, aiRequest: { ...clean.aiRequest!, scope: "episodes" as const } },
    ]) expect(getUploadSourceOptions(merge.outputPath!, [merge, other])).toEqual([{ kind: "merged", path: merge.outputPath }]);
  });

  it("offers only the processed file after merge history is removed, unless disk lookup confirms the original", () => {
    expect(getUploadSourceOptions(clean.outputs![0].path, [clean])).toEqual([{ kind: "noBackgroundMusic", path: clean.outputs![0].path }]);
    expect(getUploadSourceOptions(clean.outputs![0].path, [clean], merge.outputPath!)).toHaveLength(2);
  });
});
