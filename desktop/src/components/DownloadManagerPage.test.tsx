import { fireEvent, render, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { getDownloadStats, type DownloadManagerState } from "../download/model";
import type { DownloadManager } from "../download/useDownloadManager";
import type { MediaJobsModel } from "../media/types";
import { DownloadManagerPage } from "./DownloadManagerPage";

const state: DownloadManagerState = {
  version: 2,
  concurrency: 5,
  globallyPaused: false,
  batches: [
    {
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
        { id: "a1", itemId: "e1", episodeIndex: 1, episodeTitle: "第 1 集", definition: "1080p", status: "running", percent: 62, received: 62, total: 100, addedAt: 1 },
        { id: "a2", itemId: "e2", episodeIndex: 2, episodeTitle: "第 2 集", definition: "1080p", status: "error", percent: 0, received: 0, error: "网络错误", addedAt: 1 },
      ],
    },
    {
      id: "batch-b",
      bookId: "book-b",
      title: "女子爱财，取之有道",
      cover: "",
      series: {
        bookId: "book-b",
        seriesId: "book-b",
        title: "女子爱财，取之有道",
        cover: "",
        abstract: "",
        category: "",
        contentTypeCode: 1,
      },
      paused: false,
      createdAt: 2,
      updatedAt: 3,
      items: [
        { id: "b1", itemId: "e21", episodeIndex: 21, episodeTitle: "第 21 集", definition: "1080p", status: "done", percent: 100, received: 100, total: 100, path: "/Downloads/e21.mp4", addedAt: 2, completedAt: 3 },
      ],
    },
  ],
};

function mediaFixture(overrides: Partial<MediaJobsModel> = {}): MediaJobsModel {
  return {
    jobs: [
      {
        id: "media-queued",
        dedupeKey: "queued",
        kind: "merge",
        status: "queued",
        stage: "queued",
        percent: 0,
        inputs: [],
        outputPath: null,
        errorCode: null,
        errorMessage: null,
      },
      {
        id: "media-running",
        dedupeKey: "running",
        kind: "merge",
        status: "running",
        stage: "merging",
        percent: 42,
        inputs: [],
        outputPath: null,
        errorCode: null,
        errorMessage: null,
      },
      {
        id: "media-done",
        dedupeKey: "done",
        kind: "merge",
        status: "completed",
        stage: "completed",
        percent: 100,
        outputPath: "/Downloads/merged.mp4",
        inputs: [{ path: "/Downloads/e21.mp4", sizeBytes: 100 }],
        errorCode: null,
        errorMessage: null,
      },
      {
        id: "media-failed",
        dedupeKey: "failed",
        kind: "merge",
        status: "failed",
        stage: "failed",
        percent: 0,
        inputs: [],
        outputPath: null,
        errorCode: "FFMPEG_FAILED",
        errorMessage: "合并失败，可重试",
      },
    ],
    startMerge: vi.fn().mockResolvedValue({ id: "started" }),
    startAudioSeparation: vi.fn().mockResolvedValue({ id: "separation" }),
    startSubtitleExtraction: vi.fn().mockResolvedValue({ id: "subtitles" }),
    cancel: vi.fn().mockResolvedValue(undefined),
    retry: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
}

function managerFixture(): DownloadManager {
  return {
    state,
    stats: getDownloadStats(state),
    warning: undefined,
    enqueue: vi.fn(),
    pauseAll: vi.fn(),
    resumeAll: vi.fn(),
    setConcurrency: vi.fn(),
    pauseBatch: vi.fn(),
    resumeBatch: vi.fn(),
    retryItem: vi.fn(),
    retryBatch: vi.fn(),
    removeItem: vi.fn(),
    removeBatch: vi.fn(),
    clearCompleted: vi.fn(),
  };
}

describe("DownloadManagerPage", () => {
  it("renders one row per series and switches the fixed episode detail", () => {
    const manager = managerFixture();
    const view = render(
      <DownloadManagerPage manager={manager} media={mediaFixture()} saveDir="/Downloads/红果下载" onOpenDir={vi.fn()} onChooseDir={vi.fn()} onRevealPath={vi.fn()} />,
    );

    expect(view.getAllByTestId("download-batch-row")).toHaveLength(2);
    fireEvent.click(view.getByRole("button", { name: "查看 女子爱财，取之有道 任务详情" }));
    expect(view.getByText("第 21 集")).toBeTruthy();
    expect(view.getByText(/已完成文件/)).toBeTruthy();
  });

  it("shows aggregate statistics and exposes concurrency one through ten", () => {
    const manager = managerFixture();
    const view = render(
      <DownloadManagerPage manager={manager} media={mediaFixture()} saveDir="/Downloads/红果下载" onOpenDir={vi.fn()} onChooseDir={vi.fn()} onRevealPath={vi.fn()} />,
    );

    const stats = within(view.getByLabelText("下载统计"));
    expect(stats.getByText("下载中").parentElement?.textContent).toContain("1");
    expect(stats.getByText("失败").parentElement?.textContent).toContain("1");
    const select = view.getByLabelText("同时下载") as HTMLSelectElement;
    expect(select.value).toBe("5");
    expect(select.options).toHaveLength(10);
    fireEvent.change(select, { target: { value: "7" } });
    expect(manager.setConcurrency).toHaveBeenCalledWith(7);
  });

  it("connects global and selected-batch controls to manager actions", () => {
    const manager = managerFixture();
    const view = render(
      <DownloadManagerPage manager={manager} media={mediaFixture()} saveDir="/Downloads/红果下载" onOpenDir={vi.fn()} onChooseDir={vi.fn()} onRevealPath={vi.fn()} />,
    );

    fireEvent.click(view.getByRole("button", { name: "全部暂停" }));
    fireEvent.click(view.getByRole("button", { name: "暂停此任务" }));
    fireEvent.click(view.getByRole("button", { name: "重试失败项" }));
    fireEvent.click(view.getByRole("button", { name: "清理已完成" }));

    expect(manager.pauseAll).toHaveBeenCalledTimes(1);
    expect(manager.pauseBatch).toHaveBeenCalledWith("batch-a");
    expect(manager.retryBatch).toHaveBeenCalledWith("batch-a");
    expect(manager.clearCompleted).toHaveBeenCalledTimes(1);
  });

  it("enables merge only for completed batches and routes media cancel/retry", async () => {
    const manager = managerFixture();
    const media = mediaFixture();
    const view = render(
      <DownloadManagerPage manager={manager} media={media} saveDir="/Downloads/红果下载" onOpenDir={vi.fn()} onChooseDir={vi.fn()} onRevealPath={vi.fn()} />,
    );

    expect((view.getByRole("button", { name: "合并视频" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(view.getByRole("button", { name: "查看 女子爱财，取之有道 任务详情" }));
    fireEvent.click(view.getByRole("button", { name: "合并视频" }));
    expect(view.getByRole("dialog", { name: "合并视频" })).toBeTruthy();
    expect((view.getByRole("checkbox", { name: "转为 H.264" }) as HTMLInputElement).checked).toBe(true);
    fireEvent.click(view.getByRole("button", { name: "开始合并" }));
    await waitFor(() => expect(media.startMerge).toHaveBeenCalledWith(
      expect.objectContaining({ id: "batch-b" }),
      expect.objectContaining({ transcodeH264: true, conflictPolicy: "failIfExists" }),
    ));

    fireEvent.click(view.getByRole("tab", { name: "媒体处理" }));
    fireEvent.click(view.getByRole("button", { name: "取消" }));
    expect(media.cancel).toHaveBeenCalledWith("media-running");
    fireEvent.click(view.getByRole("button", { name: "重试" }));
    expect(media.retry).toHaveBeenCalledWith("media-failed");
    fireEvent.click(view.getByRole("tab", { name: "YouTube 上传" }));
    expect(view.getByText("尚无上传任务")).toBeTruthy();
  });

  it("routes the two AI buttons to independent jobs and offers a matching merged video", async () => {
    const media = mediaFixture();
    const view = render(
      <DownloadManagerPage manager={managerFixture()} media={media} saveDir="/Downloads/红果下载" onOpenDir={vi.fn()} onChooseDir={vi.fn()} onRevealPath={vi.fn()} />,
    );
    fireEvent.click(view.getByRole("button", { name: "查看 女子爱财，取之有道 任务详情" }));

    fireEvent.click(view.getByRole("button", { name: "分离背景音乐" }));
    expect(view.getByRole("radio", { name: "合并视频" })).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "开始分离" }));
    await waitFor(() => expect(media.startAudioSeparation).toHaveBeenCalledTimes(1));
    expect(media.startSubtitleExtraction).not.toHaveBeenCalled();

    fireEvent.click(view.getByRole("tab", { name: "下载任务" }));
    fireEvent.click(view.getByRole("button", { name: "提取字幕" }));
    fireEvent.click(view.getByRole("button", { name: "开始提取" }));
    await waitFor(() => expect(media.startSubtitleExtraction).toHaveBeenCalledTimes(1));
    expect(media.startAudioSeparation).toHaveBeenCalledTimes(1);
  });
});
