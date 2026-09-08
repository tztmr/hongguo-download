import { act, fireEvent, render, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { getDownloadStats, type DownloadManagerState } from "../download/model";
import type { DownloadManager } from "../download/useDownloadManager";
import type { YouTubeModel } from "../youtube/types";
import type { MediaJobsModel } from "../media/types";
import type { AIComponentStatus } from "../types";
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
    pause: vi.fn().mockResolvedValue(undefined),
    resume: vi.fn().mockResolvedValue(undefined),
    deleteJob: vi.fn().mockResolvedValue(undefined),
    hasMergedVideo: vi.fn().mockResolvedValue(false),
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

it("filters download tasks by failures and title and resets filters for notifications", async () => {
  const props = { manager: managerFixture(), media: mediaFixture(), saveDir: "/Downloads", onOpenDir: vi.fn(), onChooseDir: vi.fn(), onRevealPath: vi.fn() };
  const view = render(<DownloadManagerPage {...props} />);
  fireEvent.click(within(view.getByRole("group", { name: "下载状态筛选" })).getByRole("button", { name: /有失败/ }));
  expect(view.getAllByTestId("download-batch-row").map((row) => row.textContent)).toEqual([expect.stringContaining("天下第一纨绔")]);
  fireEvent.change(view.getByRole("searchbox", { name: "搜索下载任务" }), { target: { value: "女子" } });
  expect(view.getByRole("heading", { name: "没有匹配的下载任务" })).toBeTruthy();
  fireEvent.click(view.getByRole("button", { name: "显示全部下载任务" }));
  expect(view.getAllByTestId("download-batch-row")).toHaveLength(2);
  fireEvent.change(view.getByRole("searchbox", { name: "搜索下载任务" }), { target: { value: "无匹配" } });
  view.rerender(<DownloadManagerPage {...props} focusTarget={{ kind: "downloadBatch", id: "batch-b" }} />);
  await waitFor(() => {
    expect(view.getAllByTestId("download-batch-row")).toHaveLength(2);
    expect(view.getByRole("searchbox", { name: "搜索下载任务" }).getAttribute("value")).toBe("");
  });
});

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

  it("enables merge only for completed batches and routes media pause/retry", async () => {
    const manager = managerFixture();
    const media = mediaFixture();
    const view = render(
      <DownloadManagerPage manager={manager} media={media} saveDir="/Downloads/红果下载" onOpenDir={vi.fn()} onChooseDir={vi.fn()} onRevealPath={vi.fn()} />,
    );

    expect((view.getByRole("button", { name: "合并视频" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(view.getByRole("button", { name: "查看 女子爱财，取之有道 任务详情" }));
    fireEvent.click(view.getByRole("button", { name: "合并视频" }));
    expect(view.getByRole("dialog", { name: "合并视频" })).toBeTruthy();
    expect((view.getByRole("combobox", { name: "合并方式" }) as HTMLSelectElement).value).toBe("auto");
    fireEvent.click(view.getByRole("button", { name: "开始合并" }));
    await waitFor(() => expect(media.startMerge).toHaveBeenCalledWith(
      expect.objectContaining({ id: "batch-b" }),
      expect.objectContaining({ mode: "auto", quality: "high", conflictPolicy: "failIfExists" }),
    ));

    fireEvent.click(view.getByRole("tab", { name: "媒体处理" }));
    const runningRow = view.container.querySelector<HTMLElement>('[data-focus-id="media-running"]');
    expect(runningRow).toBeTruthy();
    fireEvent.click(within(runningRow as HTMLElement).getByRole("button", { name: "暂停" }));
    expect(media.pause).toHaveBeenCalledWith("media-running");
    fireEvent.click(view.getByRole("button", { name: "重试" }));
    expect(media.retry).toHaveBeenCalledWith("media-failed");
    fireEvent.click(view.getByRole("tab", { name: "YouTube 上传" }));
    expect(view.getByText("尚无上传任务")).toBeTruthy();
  });

  it("shows 已合并 and blocks the dialog when the filesystem reports a merged MP4", async () => {
    const media = mediaFixture({ hasMergedVideo: vi.fn().mockResolvedValue(true) });
    const view = render(
      <DownloadManagerPage manager={managerFixture()} media={media} saveDir="/Downloads" onOpenDir={vi.fn()} onChooseDir={vi.fn()} onRevealPath={vi.fn()} />,
    );
    fireEvent.click(view.getByRole("button", { name: "查看 女子爱财，取之有道 任务详情" }));
    await waitFor(() => expect((view.getByRole("button", { name: "已合并" }) as HTMLButtonElement).disabled).toBe(true));
    fireEvent.click(view.getByRole("button", { name: "已合并" }));
    expect(view.queryByRole("dialog", { name: "合并视频" })).toBeNull();
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

  it("keeps the chosen series fixed while a media dialog is open", async () => {
    const media = mediaFixture();
    const view = render(
      <DownloadManagerPage manager={managerFixture()} media={media} saveDir="/Downloads" onOpenDir={vi.fn()} onChooseDir={vi.fn()} onRevealPath={vi.fn()} />,
    );
    fireEvent.click(view.getByRole("button", { name: "查看 女子爱财，取之有道 任务详情" }));
    fireEvent.click(view.getByRole("button", { name: "分离背景音乐" }));

    fireEvent.click(view.getByRole("button", { name: "查看 天下第一纨绔 任务详情" }));
    const dialog = view.getByRole("dialog", { name: "分离背景音乐" });
    expect(within(dialog).getByText("女子爱财，取之有道")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "开始分离" }));

    await waitFor(() => expect(media.startAudioSeparation).toHaveBeenCalledWith(
      expect.objectContaining({ id: "batch-b" }), "episodes", "htdemucs", "/Downloads/merged.mp4",
    ));
  });

  it("does not offer another background music separation after the series completed one", () => {
    const completedSeparation = {
      ...mediaFixture().jobs[2],
      id: "separation-completed",
      kind: "separateBackgroundMusic" as const,
      aiRequest: { title: "女子爱财，取之有道", scope: "merged" as const, model: "htdemucs" as const, bookId: "book-b", seriesRoot: "/Downloads" },
    };
    const media = mediaFixture({ jobs: [...mediaFixture().jobs, completedSeparation] });
    const view = render(
      <DownloadManagerPage manager={managerFixture()} media={media} saveDir="/Downloads" onOpenDir={vi.fn()} onChooseDir={vi.fn()} onRevealPath={vi.fn()} />,
    );

    fireEvent.click(view.getByRole("button", { name: "查看 女子爱财，取之有道 任务详情" }));

    const button = view.getByRole("button", { name: "背景音乐已分离" }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    fireEvent.click(button);
    expect(view.queryByRole("dialog", { name: "分离背景音" })).toBeNull();
  });

  it("lets a media error be dismissed manually", () => {
    const media = mediaFixture({ error: { code: "MEDIA_JOB_ALREADY_ACTIVE", message: "同一媒体任务已在队列中或正在运行" } });
    const view = render(
      <DownloadManagerPage manager={managerFixture()} media={media} saveDir="/Downloads" onOpenDir={vi.fn()} onChooseDir={vi.fn()} onRevealPath={vi.fn()} />,
    );

    expect(view.getByRole("alert").textContent).toContain("同一媒体任务已在队列中或正在运行");
    fireEvent.click(view.getByRole("button", { name: "关闭提示" }));
    expect(view.queryByRole("alert")).toBeNull();
  });

  it("automatically dismisses the duplicate media task reminder after five seconds", () => {
    vi.useFakeTimers();
    try {
      const media = mediaFixture({ error: { code: "MEDIA_JOB_ALREADY_ACTIVE", message: "同一媒体任务已在队列中或正在运行" } });
      const view = render(
        <DownloadManagerPage manager={managerFixture()} media={media} saveDir="/Downloads" onOpenDir={vi.fn()} onChooseDir={vi.fn()} onRevealPath={vi.fn()} />,
      );

      act(() => vi.advanceTimersByTime(4_999));
      expect(view.getByRole("alert")).toBeTruthy();
      act(() => vi.advanceTimersByTime(1));
      expect(view.queryByRole("alert")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("filters media by status and title, then clears an empty search", () => {
    const view = render(<DownloadManagerPage manager={managerFixture()} media={mediaFixture()} saveDir="/Downloads" onOpenDir={vi.fn()} onChooseDir={vi.fn()} onRevealPath={vi.fn()} />);
    fireEvent.click(view.getByRole("tab", { name: "媒体处理" }));
    expect(view.getAllByTestId("media-job-row")).toHaveLength(4);
    fireEvent.click(view.getByRole("button", { name: /需处理/ }));
    expect(view.getAllByTestId("media-job-row")).toHaveLength(1);
    expect(view.getByText("合并失败，可重试")).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: /全部任务/ }));
    fireEvent.change(view.getByRole("searchbox", { name: "搜索媒体任务" }), { target: { value: "女子爱财" } });
    expect(view.getAllByTestId("media-job-row")).toHaveLength(1);
    fireEvent.change(view.getByRole("searchbox"), { target: { value: "没有这部剧" } });
    expect(view.getByText("没有匹配的媒体任务")).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "清除筛选" }));
    expect(view.getAllByTestId("media-job-row")).toHaveLength(4);
  });

  it("keeps saved series titles after download records are removed and shows action errors", async () => {
    const media = mediaFixture();
    media.jobs = [{ ...media.jobs[3], mergeRequest: { title: "保留的剧名" } }];
    media.retry = vi.fn().mockRejectedValue(new Error("暂时无法重试"));
    const view = render(<DownloadManagerPage manager={managerFixture()} media={media} saveDir="/Downloads" onOpenDir={vi.fn()} onChooseDir={vi.fn()} onRevealPath={vi.fn()} />);
    fireEvent.click(view.getByRole("tab", { name: "媒体处理" }));
    expect(view.getByText("保留的剧名")).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "重试" }));
    await waitFor(() => expect(view.getByRole("alert").textContent).toBe("暂时无法重试"));
  });

});


describe("YouTube separated upload source", () => {
  const youtube = {
    credential: { configured: true, clientIdSuffix: "test" },
    activeChannelId: "channel", channels: [], jobs: [],
    loading: false, busy: false, startUpload: vi.fn(),
  } as unknown as YouTubeModel;

  it.each([
    ["completed merged separation after merge record deletion", "merged", "completed", "book-b", "/Downloads", true],
    ["prefers separated MP4 over the original merge", "merged", "completed", "book-b", "/Downloads", true, true],
    ["legacy separation linked to retained merge", "merged", "completed", undefined, undefined, true, true],
    ["episode separation", "episodes", "completed", "book-b", "/Downloads", false],
    ["unfinished separation", "merged", "running", "book-b", "/Downloads", false],
    ["another book in same directory", "merged", "completed", "other", "/Downloads", false],
    ["same book in another directory", "merged", "completed", "book-b", "/Elsewhere", false],
  ] as const)("handles %s", async (_, scope, status, bookId, seriesRoot, enabled, retainMerge: boolean = false) => {
    const separation = {
      ...mediaFixture().jobs[2], kind: "separateBackgroundMusic" as const, status,
      aiRequest: { title: "女子爱财，取之有道", scope, model: "htdemucs" as const, bookId, seriesRoot },
      inputs: [{ path: scope === "episodes" ? "/Downloads/e21.mp4" : "/Downloads/merged.mp4", sizeBytes: 100 }],
      outputPath: "/Downloads/音频分离/人声.wav",
      outputs: [{ episodeIndex: 1, kind: "noBackgroundMusicVideo" as const, path: "/Downloads/音频分离/去背景音乐.mp4" }],
    };
    const view = render(<DownloadManagerPage manager={managerFixture()} media={mediaFixture({ jobs: [...(retainMerge ? [mediaFixture().jobs[2]] : []), separation] })}
      youtube={youtube} saveDir="/Downloads" onOpenDir={vi.fn()} onChooseDir={vi.fn()} onRevealPath={vi.fn()} />);
    fireEvent.click(view.getByRole("button", { name: "查看 女子爱财，取之有道 任务详情" }));
    const button = view.getByRole("button", { name: "上传 YouTube" }) as HTMLButtonElement;
    await waitFor(() => expect(button.disabled).toBe(!enabled));
    if (enabled) {
      fireEvent.click(button);
      expect(within(view.getByRole("dialog", { name: "上传到 YouTube" })).getByText("上传文件：/Downloads/音频分离/去背景音乐.mp4")).toBeTruthy();
      expect(youtube.startUpload).not.toHaveBeenCalled();
    }
  });

  it("offers YouTube upload directly on a completed background separation job", () => {
    const separation = {
      ...mediaFixture().jobs[2], id: "separation-upload", kind: "separateBackgroundMusic" as const,
      aiRequest: { title: "女子爱财，取之有道", scope: "merged" as const, model: "htdemucs" as const, bookId: "book-b", seriesRoot: "/Downloads" },
      inputs: [{ path: "/Downloads/merged.mp4", sizeBytes: 100 }],
      outputs: [{ episodeIndex: 1, kind: "noBackgroundMusicVideo" as const, path: "/Downloads/音频分离/去背景音乐.mp4" }],
    };
    const view = render(<DownloadManagerPage manager={managerFixture()} media={mediaFixture({ jobs: [separation] })}
      youtube={youtube} saveDir="/Downloads" onOpenDir={vi.fn()} onChooseDir={vi.fn()} onRevealPath={vi.fn()} />);

    fireEvent.click(view.getByRole("tab", { name: "媒体处理" }));
    fireEvent.click(within(view.getByTestId("media-job-row")).getByRole("button", { name: "上传 YouTube" }));

    const dialog = view.getByRole("dialog", { name: "上传到 YouTube" });
    expect(within(dialog).getByText("上传文件：/Downloads/音频分离/去背景音乐.mp4")).toBeTruthy();
    expect((within(dialog).getByLabelText("YouTube 标题") as HTMLInputElement).value).toBe("女子爱财，取之有道");
  });
});

function aiComponent(id: string, installed = false): AIComponentStatus {
  return {
    id,
    version: "1",
    installed,
    installedVersion: installed ? "1" : null,
    installedPath: null,
    downloadBytes: 1024,
    installedBytes: 2048,
    inUse: false,
  };
}

const windowsCatalog = [
  aiComponent("runtime-modern"),
  aiComponent("runtime-legacy"),
  aiComponent("runtime-cpu"),
  aiComponent("demucs-htdemucs"),
  aiComponent("whisper-small"),
];

describe("Windows post-merge media actions", () => {
  const youtube = {
    credential: { configured: true, clientIdSuffix: "test" },
    activeChannelId: "channel", channels: [], jobs: [],
    loading: false, busy: false, startUpload: vi.fn(),
  } as unknown as YouTubeModel;

  function windowsManager(): DownloadManager {
    const windowsState: DownloadManagerState = {
      ...state,
      batches: [
        state.batches[0],
        {
          ...state.batches[1],
          items: [{ ...state.batches[1].items[0], path: "C:\\Users\\edking\\Downloads\\e21.mp4" }],
        },
      ],
    };
    return {
      ...managerFixture(),
      state: windowsState,
      stats: getDownloadStats(windowsState),
    };
  }

  const windowsMerge = {
    ...mediaFixture().jobs[2],
    outputPath: "\\\\?\\C:\\Users\\edking\\Downloads\\merged.mp4",
    inputs: [{ path: "\\\\?\\C:\\Users\\edking\\Downloads\\e21.mp4", sizeBytes: 100 }],
    mergeRequest: {
      title: "女子爱财，取之有道",
      bookId: "book-b",
      seriesRoot: "\\\\?\\C:\\Users\\edking\\Downloads",
    },
  };

  it("offers 合并视频 and YouTube after a Windows merge whose paths use the \\\\?\\ prefix", async () => {
    const view = render(
      <DownloadManagerPage
        manager={windowsManager()}
        media={mediaFixture({ jobs: [windowsMerge] })}
        youtube={youtube}
        saveDir="C:\\Users\\edking\\Downloads"
        onOpenDir={vi.fn()}
        onChooseDir={vi.fn()}
        onRevealPath={vi.fn()}
      />,
    );
    fireEvent.click(view.getByRole("button", { name: "查看 女子爱财，取之有道 任务详情" }));
    fireEvent.click(view.getByRole("button", { name: "分离背景音乐" }));
    expect(view.getByRole("radio", { name: "合并视频" })).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "取消" }));
    const button = view.getByRole("button", { name: "上传 YouTube" }) as HTMLButtonElement;
    await waitFor(() => expect(button.disabled).toBe(false));
    expect(button.title).toBe("");
  });

  it("starts background separation when runtime-modern and the model are already installed", async () => {
    const media = mediaFixture();
    const view = render(
      <DownloadManagerPage
        manager={managerFixture()}
        media={media}
        aiComponents={[aiComponent("runtime-modern", true), aiComponent("demucs-htdemucs", true)]}
        onInstallComponent={vi.fn()}
        saveDir="/Downloads"
        onOpenDir={vi.fn()}
        onChooseDir={vi.fn()}
        onRevealPath={vi.fn()}
      />,
    );
    fireEvent.click(view.getByRole("button", { name: "查看 女子爱财，取之有道 任务详情" }));
    fireEvent.click(view.getByRole("button", { name: "分离背景音乐" }));
    fireEvent.click(view.getByRole("button", { name: "开始分离" }));
    await waitFor(() => expect(media.startAudioSeparation).toHaveBeenCalledTimes(1));
    expect(view.queryByRole("dialog", { name: "安装 AI 组件" })).toBeNull();
  });

  it("asks Windows catalogs to install runtime-modern instead of an unconfigured runtime", async () => {
    const media = mediaFixture();
    const view = render(
      <DownloadManagerPage
        manager={managerFixture()}
        media={media}
        aiComponents={windowsCatalog}
        onInstallComponent={vi.fn()}
        saveDir="/Downloads"
        onOpenDir={vi.fn()}
        onChooseDir={vi.fn()}
        onRevealPath={vi.fn()}
      />,
    );
    fireEvent.click(view.getByRole("button", { name: "查看 女子爱财，取之有道 任务详情" }));
    fireEvent.click(view.getByRole("button", { name: "分离背景音乐" }));
    fireEvent.click(view.getByRole("button", { name: "开始分离" }));
    const dialog = await waitFor(() => view.getByRole("dialog", { name: "安装 AI 组件" }));
    expect(dialog.textContent).toContain("runtime-modern");
    expect(dialog.textContent).not.toContain("runtime · 未在发布清单中配置");
    expect(dialog.textContent).not.toContain("未在发布清单中配置");
    expect(media.startAudioSeparation).not.toHaveBeenCalled();
    expect((within(dialog).getByRole("button", { name: "确认安装并继续" }) as HTMLButtonElement).disabled).toBe(false);
  });
});
