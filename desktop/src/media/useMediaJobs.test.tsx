import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MediaJob, StartMergeRequest } from "./types";
import { useMediaJobs } from "./useMediaJobs";

const queuedJob: MediaJob = {
  id: "job-1",
  dedupeKey: "merge-1",
  kind: "merge",
  status: "queued",
  stage: "queued",
  percent: 0,
  inputs: [{ path: "/Downloads/e1.mp4", sizeBytes: 12 }],
  outputPath: null,
  errorCode: null,
  errorMessage: null,
};

const runningJob: MediaJob = {
  ...queuedJob,
  status: "running",
  stage: "merging",
  percent: 40,
};

const completedJob: MediaJob = {
  ...queuedJob,
  status: "completed",
  stage: "completed",
  percent: 100,
  outputPath: "/Downloads/合并视频/output.mp4",
};

const pausedJob: MediaJob = {
  ...runningJob,
  status: "paused",
  pauseOrigin: "running",
  stage: "paused",
};

function createCommands() {
  const unlisten = vi.fn();
  let listener: ((job: MediaJob) => void) | undefined;
  return {
    snapshot: vi.fn().mockResolvedValue({ version: 1, jobs: [queuedJob], warning: null }),
    startMerge: vi.fn().mockResolvedValue(queuedJob),
    startAudioSeparation: vi.fn().mockResolvedValue({ ...queuedJob, kind: "separateBackgroundMusic" }),
    startSubtitleExtraction: vi.fn().mockResolvedValue({ ...queuedJob, kind: "extractSubtitles" }),
    cancel: vi.fn().mockResolvedValue(undefined),
    pause: vi.fn().mockResolvedValue(pausedJob),
    resume: vi.fn().mockResolvedValue(runningJob),
    deleteJob: vi.fn().mockResolvedValue(undefined),
    hasMergedVideo: vi.fn().mockResolvedValue(true),
    retry: vi.fn().mockResolvedValue(queuedJob),
    subscribeProgress: vi.fn(async (next: (job: MediaJob) => void) => {
      listener = next;
      return unlisten;
    }),
    emit(job: MediaJob) {
      listener?.(job);
    },
    unlisten,
  };
}

afterEach(() => {
  vi.resetModules();
});

describe("useMediaJobs", () => {
  it("loads the native snapshot once and replaces jobs by id from complete progress events", async () => {
    const commands = createCommands();
    const { result, unmount } = renderHook(() => useMediaJobs({ commands }));

    await waitFor(() => expect(result.current.jobs).toEqual([queuedJob]));
    act(() => commands.emit(runningJob));
    await waitFor(() => expect(result.current.jobs).toEqual([runningJob]));
    act(() => commands.emit(completedJob));
    await waitFor(() => expect(result.current.jobs).toEqual([completedJob]));
    expect(commands.snapshot).toHaveBeenCalledTimes(1);
    unmount();
    expect(commands.unlisten).toHaveBeenCalledTimes(1);
  });

  it("submits episode-sorted completed paths and surfaces duplicate-active errors by stable code", async () => {
    const commands = createCommands();
    commands.startMerge.mockRejectedValue({
      code: "MEDIA_JOB_ALREADY_ACTIVE",
      message: "同一媒体任务已在队列中或正在运行",
    });
    const { result } = renderHook(() => useMediaJobs({ commands }));
    await waitFor(() => expect(result.current.jobs).toHaveLength(1));

    const batch = {
      id: "batch-a",
      bookId: "book-a",
      title: "天下第一纨绔",
      cover: "",
      series: {
        bookId: "book-a",
        seriesId: "book-a",
        title: "天下第一纨绔",
        cover: "",
        abstract: "",
        category: "",
        contentTypeCode: 1,
      },
      paused: false,
      createdAt: 1,
      updatedAt: 2,
      items: [
        { id: "a2", itemId: "e2", episodeIndex: 2, episodeTitle: "第 2 集", definition: "1080p", status: "done" as const, percent: 100, received: 2, path: "/Downloads/e2.mp4", addedAt: 1 },
        { id: "a1", itemId: "e1", episodeIndex: 1, episodeTitle: "第 1 集", definition: "1080p", status: "done" as const, percent: 100, received: 1, path: "/Downloads/e1.mp4", addedAt: 1 },
      ],
    };

    await act(async () => {
      await expect(
        result.current.startMerge(batch, {
          outputName: "天下第一纨绔.mp4",
          mode: "auto",
          quality: "balanced",
          conflictPolicy: "failIfExists",
        }),
      ).rejects.toMatchObject({ code: "MEDIA_JOB_ALREADY_ACTIVE" });
    });

    const request = commands.startMerge.mock.calls[0][0] as StartMergeRequest;
    expect(request).toMatchObject({
      bookId: "book-a",
      title: "天下第一纨绔",
      outputFileName: "天下第一纨绔.mp4",
      mode: "auto",
      quality: "balanced",
      conflictPolicy: "failIfExists",
      inputs: [
        { episodeIndex: 1, path: "/Downloads/e1.mp4" },
        { episodeIndex: 2, path: "/Downloads/e2.mp4" },
      ],
    });
    expect(request.inputs[0]).not.toHaveProperty("size");
    expect(request.inputs[0]).not.toHaveProperty("modifiedUnixNanos");
    expect(result.current.error?.code).toBe("MEDIA_JOB_ALREADY_ACTIVE");
  });

  it("does not invoke native commands when disabled and still routes injected preview actions", async () => {
    const commands = createCommands();
    const { result, unmount } = renderHook(() =>
      useMediaJobs({ commands, enabled: false, initialJobs: [runningJob] }),
    );
    expect(result.current.jobs).toEqual([runningJob]);
    await act(async () => {
      await result.current.cancel("job-1");
    });
    expect(commands.snapshot).not.toHaveBeenCalled();
    expect(commands.subscribeProgress).not.toHaveBeenCalled();
    expect(commands.cancel).toHaveBeenCalledWith("job-1");
    unmount();
  });

  it("rejects incomplete batches before calling startMerge", async () => {
    const commands = createCommands();
    const { result } = renderHook(() => useMediaJobs({ commands }));
    await waitFor(() => expect(result.current.jobs).toHaveLength(1));
    const batch = {
      id: "batch-partial",
      bookId: "book-a",
      title: "天下第一纨绔",
      cover: "",
      series: {
        bookId: "book-a",
        seriesId: "book-a",
        title: "天下第一纨绔",
        cover: "",
        abstract: "",
        category: "",
        contentTypeCode: 1,
      },
      paused: false,
      createdAt: 1,
      updatedAt: 2,
      items: [
        { id: "a1", itemId: "e1", episodeIndex: 1, episodeTitle: "第 1 集", definition: "1080p", status: "done" as const, percent: 100, received: 1, path: "/Downloads/e1.mp4", addedAt: 1 },
        { id: "a2", itemId: "e2", episodeIndex: 2, episodeTitle: "第 2 集", definition: "1080p", status: "running" as const, percent: 40, received: 1, addedAt: 1 },
      ],
    };
    await act(async () => {
      await expect(
        result.current.startMerge(batch, {
          outputName: "天下第一纨绔.mp4",
          transcodeH264: true,
          conflictPolicy: "failIfExists",
        }),
      ).rejects.toMatchObject({ code: "MERGE_INPUT_INVALID" });
    });
    expect(commands.startMerge).not.toHaveBeenCalled();
  });

  it("routes cancel and retry through native job ids without duplicating the scheduler", async () => {
    const commands = createCommands();
    const { result } = renderHook(() => useMediaJobs({ commands }));
    await waitFor(() => expect(result.current.jobs).toHaveLength(1));

    await act(async () => {
      await result.current.cancel("job-1");
      await result.current.retry("job-1");
    });

    expect(commands.cancel).toHaveBeenCalledWith("job-1");
    expect(commands.retry).toHaveBeenCalledWith("job-1");
  });

  it("updates pause and resume results, deletes the returned job, and queries merged files", async () => {
    const commands = createCommands();
    const { result } = renderHook(() => useMediaJobs({ commands }));
    await waitFor(() => expect(result.current.jobs).toEqual([queuedJob]));

    await act(async () => {
      await result.current.pause("job-1");
    });
    expect(result.current.jobs[0]).toEqual(pausedJob);
    await act(async () => {
      await result.current.resume("job-1");
    });
    expect(result.current.jobs[0]).toEqual(runningJob);
    await act(async () => {
      await result.current.deleteJob("job-1");
    });
    expect(result.current.jobs).toEqual([]);
    await expect(result.current.hasMergedVideo("/Downloads/series")).resolves.toBe(true);
  });

  it("restores a deleted job when the native delete command fails", async () => {
    const commands = createCommands();
    commands.deleteJob.mockRejectedValue({ code: "MEDIA_JOB_SIGNAL_FAILED", message: "无法停止进程" });
    const { result } = renderHook(() => useMediaJobs({ commands }));
    await waitFor(() => expect(result.current.jobs).toEqual([queuedJob]));

    await act(async () => {
      await expect(result.current.deleteJob("job-1")).rejects.toMatchObject({ code: "MEDIA_JOB_SIGNAL_FAILED" });
    });
    expect(result.current.jobs).toEqual([queuedJob]);
  });

  it("routes separation and subtitle actions to distinct native commands", async () => {
    const commands = createCommands();
    const { result } = renderHook(() => useMediaJobs({ commands }));
    await waitFor(() => expect(result.current.jobs).toHaveLength(1));
    const batch = {
      id: "batch-ai", bookId: "book-ai", title: "AI 剧", cover: "", paused: false,
      series: { bookId: "book-ai", seriesId: "book-ai", title: "AI 剧", cover: "", abstract: "", category: "", contentTypeCode: 1 },
      createdAt: 1, updatedAt: 2,
      items: [{ id: "a1", itemId: "e1", episodeIndex: 1, episodeTitle: "第 1 集", definition: "1080p", status: "done" as const, percent: 100, received: 1, path: "/Downloads/AI 剧/e1.mp4", addedAt: 1 }],
    };

    await act(async () => {
      await result.current.startAudioSeparation(batch, "episodes", "htdemucs");
      await result.current.startSubtitleExtraction(batch, "episodes", "small");
    });

    expect(commands.startAudioSeparation).toHaveBeenCalledTimes(1);
    expect(commands.startSubtitleExtraction).toHaveBeenCalledTimes(1);
    expect(commands.startAudioSeparation.mock.calls[0][0]).toMatchObject({ scope: "episodes", model: "htdemucs" });
    expect(commands.startSubtitleExtraction.mock.calls[0][0]).toMatchObject({ scope: "episodes", model: "small" });
  });
});
