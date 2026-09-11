import { act, fireEvent, render, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { MediaJob, MediaJobsModel } from "../media/types";
import { MediaJobsPanel } from "./MediaJobsPanel";

function job(id: string, overrides: Partial<MediaJob> = {}): MediaJob {
  return {
    id, dedupeKey: id, kind: "merge", status: "completed", stage: "completed", percent: 100,
    mergeRequest: { title: id }, inputs: [{ path: `/source/${id}.mp4`, sizeBytes: 100 }],
    outputPath: `/output/${id}.mp4`, errorCode: null, errorMessage: null, ...overrides,
  };
}
function model(jobs: MediaJob[], overrides: Partial<MediaJobsModel> = {}): MediaJobsModel {
  return {
    jobs, startMerge: vi.fn(), startAudioSeparation: vi.fn(), startSubtitleExtraction: vi.fn(),
    cancel: vi.fn(), pause: vi.fn().mockResolvedValue(undefined), resume: vi.fn(), retry: vi.fn(),
    deleteJob: vi.fn().mockResolvedValue(undefined), hasMergedVideo: vi.fn(), findMergedVideo: vi.fn(), ...overrides,
  };
}
const baseProps = { batches: [], onRevealPath: vi.fn(), onShowDownloads: vi.fn() };
const selected = (element: HTMLElement) => (element as HTMLInputElement).checked;
const disabled = (element: HTMLElement) => (element as HTMLButtonElement).disabled;

function separated(id: string, overrides: Partial<MediaJob> = {}) {
  return job(id, {
    kind: "separateBackgroundMusic", mergeRequest: null, outputPath: "/output",
    aiRequest: { title: id, scope: "merged", model: "htdemucs" },
    outputs: [{ kind: "vocals", episodeIndex: 0, path: "/output/vocals.wav" },
      { kind: "noBackgroundMusicVideo", episodeIndex: 0, path: `/output/${id}-clean.mp4` }], ...overrides,
  });
}

describe("media list selection", () => {
  it("groups merge, background music, and subtitle tasks with type filters", () => {
    const jobs = [
      job("Merged", { status: "completed" }),
      job("Queued merge", { status: "queued", stage: "queued", percent: 0 }),
      separated("Cleaned"),
      job("Subtitles", { kind: "extractSubtitles", outputPath: "/output/Subtitles.srt", aiRequest: { title: "Subtitles", scope: "merged", model: "small" } }),
    ];
    const view = render(<MediaJobsPanel {...baseProps} media={model(jobs)} />);
    expect(view.getByRole("button", { name: /合并成功.*1/ })).toBeTruthy();
    expect(view.getByRole("button", { name: /分离背景音乐.*1/ })).toBeTruthy();
    expect(view.getByRole("button", { name: /提取字幕.*1/ })).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: /分离背景音乐.*1/ }));
    expect(view.getAllByTestId("media-job-row")).toHaveLength(1);
    expect(view.getByText("Cleaned")).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: /提取字幕.*1/ }));
    expect(view.getAllByTestId("media-job-row")).toHaveLength(1);
    expect(view.getByText("Subtitles")).toBeTruthy();
  });

  it("pauses and resumes only eligible selected visible tasks in bulk", async () => {
    const queued = job("Queued", { status: "queued", stage: "queued", percent: 0 });
    const running = job("Running", { status: "running", stage: "running", percent: 35 });
    const paused = job("Paused", { status: "paused", stage: "paused", percent: 35 });
    const completed = job("Completed");
    const media = model([queued, running, paused, completed]);
    const onNotice = vi.fn();
    const view = render(<MediaJobsPanel {...baseProps} media={media} onNotice={onNotice} />);
    fireEvent.click(view.getByRole("checkbox", { name: "全选当前可见媒体任务" }));
    expect(disabled(view.getByRole("button", { name: /批量暂停/ }))).toBe(false);
    expect(disabled(view.getByRole("button", { name: /批量开启/ }))).toBe(false);
    fireEvent.click(view.getByRole("button", { name: /批量暂停/ }));
    await waitFor(() => {
      expect(media.pause).toHaveBeenCalledTimes(2);
      expect(media.pause).toHaveBeenCalledWith("Queued");
      expect(media.pause).toHaveBeenCalledWith("Running");
    });
    expect(onNotice).toHaveBeenCalledWith("已暂停 2 项媒体任务");
    fireEvent.click(view.getByRole("button", { name: /批量开启/ }));
    await waitFor(() => {
      expect(media.resume).toHaveBeenCalledTimes(1);
      expect(media.resume).toHaveBeenCalledWith("Paused");
    });
    expect(onNotice).toHaveBeenCalledWith("已开启 1 项媒体任务");
  });

  it("renders subtitle model stages and advances decoded-audio progress", () => {
    const subtitle = job("字幕识别", { kind: "extractSubtitles", status: "running", stage: "第 1 集 · loadingSubtitleModel", percent: 2 });
    const view = render(<MediaJobsPanel {...baseProps} media={model([subtitle])} />);
    expect(view.getByText("第 1 集 · 加载字幕模型")).toBeTruthy();
    view.rerender(<MediaJobsPanel {...baseProps} media={model([{ ...subtitle, stage: "第 1 集 · 识别字幕 · 已处理 00:00:30 / 00:01:30", percent: 35 }])} />);
    expect(view.getByRole("progressbar", { name: "提取字幕进度" }).getAttribute("aria-valuenow")).toBe("35");
    expect(view.getByText(/已处理 00:00:30 \/ 00:01:30/)).toBeTruthy();
  });

  it("keeps hidden selection and deletes only selected visible rows without clearing filters", async () => {
    const media = model([job("Alpha"), job("Beta", { status: "failed" })]);
    const onNotice = vi.fn();
    const view = render(<MediaJobsPanel {...baseProps} media={media} onNotice={onNotice} />);
    expect(disabled(view.getByRole("button", { name: "批量删除" }))).toBe(true);
    fireEvent.click(view.getByRole("checkbox", { name: "全选当前可见媒体任务" }));
    fireEvent.click(within(view.getByRole("group", { name: "媒体任务状态" })).getByRole("button", { name: /已完成/ }));
    fireEvent.change(view.getByRole("searchbox"), { target: { value: "Alpha" } });
    expect(view.getByText("已选 1 项（当前可见），共选 2 项")).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "批量删除" }));
    await waitFor(() => expect(media.deleteJob).toHaveBeenCalledExactlyOnceWith("Alpha"));
    await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.stringContaining("已删除 1 项")));
    expect((view.getByRole("searchbox") as HTMLInputElement).value).toBe("Alpha");
    expect(view.getByText("已选 0 项（当前可见），共选 1 项")).toBeTruthy();
    fireEvent.change(view.getByRole("searchbox"), { target: { value: "" } });
    fireEvent.click(within(view.getByRole("group", { name: "媒体任务状态" })).getByRole("button", { name: /全部任务/ }));
    expect(selected(view.getByRole("checkbox", { name: "选择媒体任务：Beta" }))).toBe(true);
    expect(selected(view.getByRole("checkbox", { name: "选择媒体任务：Alpha" }))).toBe(false);
  });

  it("selects and clears only visible rows and exposes mixed select-all state", () => {
    const media = model([job("Alpha"), job("Beta")]);
    const view = render(<MediaJobsPanel {...baseProps} media={media} />);
    fireEvent.click(view.getByRole("checkbox", { name: "选择媒体任务：Alpha" }));
    expect((view.getByRole("checkbox", { name: "全选当前可见媒体任务" }) as HTMLInputElement).indeterminate).toBe(true);
    fireEvent.change(view.getByRole("searchbox"), { target: { value: "Beta" } });
    fireEvent.click(view.getByRole("checkbox", { name: "全选当前可见媒体任务" }));
    fireEvent.click(view.getByRole("checkbox", { name: "全选当前可见媒体任务" }));
    expect(view.getByText("已选 0 项（当前可见），共选 1 项")).toBeTruthy();
    fireEvent.change(view.getByRole("searchbox"), { target: { value: "missing" } });
    expect(disabled(view.getByRole("checkbox", { name: "全选当前可见媒体任务" }))).toBe(true);
    expect(disabled(view.getByRole("button", { name: "批量删除" }))).toBe(true);
    view.rerender(<MediaJobsPanel {...baseProps} media={{ ...media, jobs: [job("Beta")] }} />);
    expect(view.getByText("已选 0 项（当前可见），共选 0 项")).toBeTruthy();
  });

  it("guards pending deletion, deselects successes and retains failures for retry", async () => {
    let finish!: () => void;
    const deleteJob = vi.fn().mockImplementation((id: string) => id === "Alpha"
      ? new Promise<void>((resolve) => { finish = resolve; })
      : Promise.reject({ message: "无法停止进程" }));
    const onNotice = vi.fn();
    const view = render(<MediaJobsPanel {...baseProps} media={model([job("Alpha"), job("Beta")], { deleteJob })} onNotice={onNotice} />);
    fireEvent.click(view.getByRole("checkbox", { name: "全选当前可见媒体任务" }));
    const bulk = view.getByRole("button", { name: "批量删除" });
    fireEvent.click(bulk);
    fireEvent.click(bulk);
    expect(disabled(bulk)).toBe(true);
    for (const row of view.getAllByTestId("media-job-row")) {
      expect(disabled(within(row).getByRole("button", { name: "删除" }))).toBe(true);
      fireEvent.click(within(row).getByRole("button", { name: "删除" }));
    }
    await act(async () => finish());
    await waitFor(() => expect(disabled(bulk)).toBe(false));
    expect(deleteJob.mock.calls).toEqual([["Alpha"], ["Beta"]]);
    expect(selected(view.getByRole("checkbox", { name: "选择媒体任务：Alpha" }))).toBe(false);
    expect(selected(view.getByRole("checkbox", { name: "选择媒体任务：Beta" }))).toBe(true);
    expect(view.getByRole("alert").textContent).toContain("1 项删除失败");
    expect(view.getByRole("alert").textContent).toContain("无法停止进程");
    expect(onNotice).toHaveBeenCalledWith(expect.stringContaining("已删除 1 项"));
    deleteJob.mockResolvedValue(undefined);
    fireEvent.click(bulk);
    await waitFor(() => expect(view.getByText("已选 0 项（当前可见），共选 0 项")).toBeTruthy());
    expect(deleteJob.mock.calls).toEqual([["Alpha"], ["Beta"], ["Beta"]]);
  });

  it("blocks bulk deletion while a selected row action is pending and notifies single deletion", async () => {
    let finish!: () => void;
    const media = model([job("Alpha", { status: "running" })], { pause: vi.fn(() => new Promise<void>((resolve) => { finish = resolve; })) });
    const onNotice = vi.fn();
    const view = render(<MediaJobsPanel {...baseProps} media={media} onNotice={onNotice} />);
    fireEvent.click(view.getByRole("checkbox", { name: "选择媒体任务：Alpha" }));
    fireEvent.click(view.getByRole("button", { name: "暂停" }));
    expect(disabled(view.getByRole("button", { name: "批量删除" }))).toBe(true);
    await act(async () => finish());
    fireEvent.click(view.getByRole("button", { name: "删除" }));
    await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.stringContaining("已删除 1 项")));
    expect(selected(view.getByRole("checkbox", { name: "选择媒体任务：Alpha" }))).toBe(false);
    expect(media.deleteJob).toHaveBeenCalledExactlyOnceWith("Alpha");
  });
});

describe("media follow-up actions", () => {
  it("offers separation then upload for a completed merge and uploads the clean video for merged separation", () => {
    const merge = job("Merge");
    const clean = separated("Clean");
    const onSeparateVideo = vi.fn();
    const onUploadToYouTube = vi.fn();
    const view = render(<MediaJobsPanel {...baseProps} media={model([merge, clean])} onSeparateVideo={onSeparateVideo} onUploadToYouTube={onUploadToYouTube} />);
    const [mergeRow, cleanRow] = view.getAllByTestId("media-job-row").map((row) => within(row));
    const names = mergeRow.getAllByRole("button").map((button) => button.textContent);
    expect(names.slice(names.indexOf("定位"), names.indexOf("删除"))).toEqual(["定位", "分离视频", "上传 YouTube"]);
    fireEvent.click(mergeRow.getByRole("button", { name: "分离视频" }));
    fireEvent.click(mergeRow.getByRole("button", { name: "上传 YouTube" }));
    fireEvent.click(cleanRow.getByRole("button", { name: "上传 YouTube" }));
    expect(onSeparateVideo).toHaveBeenCalledExactlyOnceWith(merge, "/output/Merge.mp4");
    expect(onUploadToYouTube.mock.calls).toEqual([[merge, "/output/Merge.mp4"], [clean, "/output/Clean-clean.mp4"]]);
    expect(cleanRow.queryByRole("button", { name: "分离视频" })).toBeNull();
  });

  it("disables follow-ups without callbacks and displays per-job disabled reasons", () => {
    const media = model([job("Merge")]);
    const view = render(<MediaJobsPanel {...baseProps} media={media} />);
    for (const name of ["分离视频", "上传 YouTube", "批量上传 YouTube"]) {
      expect(disabled(view.getByRole("button", { name }))).toBe(true);
    }
    const onSeparateVideo = vi.fn();
    const onUploadToYouTube = vi.fn();
    view.rerender(<MediaJobsPanel {...baseProps} media={media} onSeparateVideo={onSeparateVideo} onUploadToYouTube={onUploadToYouTube}
      separationDisabledReason={() => "已有分离任务"} youtubeUploadDisabledReason={() => "请先授权频道"} />);
    const separate = view.getByRole("button", { name: "分离视频" });
    const upload = view.getByRole("button", { name: "上传 YouTube" });
    expect(disabled(separate)).toBe(true);
    expect(separate.title).toBe("已有分离任务");
    expect(disabled(upload)).toBe(true);
    expect(upload.title).toBe("请先授权频道");
    fireEvent.click(separate);
    fireEvent.click(upload);
    expect(onSeparateVideo).not.toHaveBeenCalled();
    expect(onUploadToYouTube).not.toHaveBeenCalled();
  });

  it("bulk uploads only selected visible eligible outputs and reports skipped rows", () => {
    const merge = job("Shown merge");
    const clean = separated("Shown clean");
    const jobs = [merge, clean, job("Shown blocked"), job("Shown running", { status: "running" }),
      separated("Shown episodes", { aiRequest: { title: "Shown episodes", scope: "episodes", model: "htdemucs" } }),
      job("Hidden"), job("Shown unselected")];
    const onBulkUploadToYouTube = vi.fn();
    const media = model(jobs);
    const props = { ...baseProps, media, onBulkUploadToYouTube, youtubeUploadDisabledReason: (item: MediaJob) => item.id === "Shown blocked" ? "重复上传" : undefined };
    const view = render(<MediaJobsPanel {...props} />);
    fireEvent.click(view.getByRole("checkbox", { name: "全选当前可见媒体任务" }));
    fireEvent.click(view.getByRole("checkbox", { name: "选择媒体任务：Shown unselected" }));
    fireEvent.change(view.getByRole("searchbox"), { target: { value: "Shown" } });
    expect(view.getByText(/可上传 2 项.*跳过 3 项/)).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "批量上传 YouTube" }));
    expect(onBulkUploadToYouTube).toHaveBeenCalledExactlyOnceWith([
      { job: merge, sourcePath: "/output/Shown merge.mp4" }, { job: clean, sourcePath: "/output/Shown clean-clean.mp4" },
    ]);
    expect(view.getByText("已选 5 项（当前可见），共选 6 项")).toBeTruthy();
    view.rerender(<MediaJobsPanel {...props} onBulkUploadToYouTube={undefined} />);
    expect(disabled(view.getByRole("button", { name: "批量上传 YouTube" }))).toBe(true);
    view.rerender(<MediaJobsPanel {...props} youtubeUploadDisabledReason={() => "请授权"} />);
    expect(disabled(view.getByRole("button", { name: "批量上传 YouTube" }))).toBe(true);
    expect(view.getByText(/可上传 0 项.*跳过 5 项/)).toBeTruthy();
  });

  it("does not offer follow-ups for incomplete, missing-output, subtitle or episode jobs", () => {
    const jobs = [job("Running", { status: "running" }), job("Missing", { outputPath: null }),
      separated("Episodes", { aiRequest: { title: "Episodes", scope: "episodes", model: "htdemucs" } }),
      separated("No video", { outputs: [{ kind: "vocals", episodeIndex: 0, path: "/vocals.wav" }] }),
      job("Subtitles", { kind: "extractSubtitles", outputPath: "/subtitle.srt" })];
    const view = render(<MediaJobsPanel {...baseProps} media={model(jobs)} onSeparateVideo={vi.fn()} onUploadToYouTube={vi.fn()} />);
    expect(view.queryByRole("button", { name: "分离视频" })).toBeNull();
    expect(view.queryByRole("button", { name: "上传 YouTube" })).toBeNull();
  });
});
