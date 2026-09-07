import { fireEvent, render, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { previewYouTubeModel } from "../preview";
import { YouTubeUploadJobs } from "./YouTubeUploadJobs";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: vi.fn() }));
const completed = { ...previewYouTubeModel, jobs: [previewYouTubeModel.jobs[1]] };

it("combines upload status and title filters and reveals notification targets", () => {
  const view = render(<YouTubeUploadJobs model={previewYouTubeModel} onRevealPath={vi.fn()} />);
  fireEvent.click(within(view.getByRole("group", { name: "上传状态筛选" })).getByRole("button", { name: /待处理/ }));
  expect(view.getAllByTestId("youtube-job-row")).toHaveLength(1);
  expect(view.getByRole("button", { name: "仅重试封面" })).toBeTruthy();
  fireEvent.change(view.getByRole("searchbox", { name: "搜索上传任务" }), { target: { value: "无匹配" } });
  expect(view.getByRole("heading", { name: "没有匹配的上传任务" })).toBeTruthy();
  view.rerender(<YouTubeUploadJobs model={previewYouTubeModel} onRevealPath={vi.fn()} focusJobId={previewYouTubeModel.jobs[0].id} />);
  expect(view.getAllByTestId("youtube-job-row")).toHaveLength(3);
});

beforeEach(() => {
  vi.mocked(isTauri).mockReturnValue(true);
  vi.mocked(invoke).mockReset().mockResolvedValue(undefined);
});

describe("YouTube video link", () => {
  it("opens the completed video through the desktop opener and prevents webview navigation", async () => {
    const view = render(<YouTubeUploadJobs model={completed} onRevealPath={vi.fn()} />);
    const nativeNavigation = fireEvent.click(view.getByRole("link", { name: "打开 YouTube 视频" }));
    expect(nativeNavigation).toBe(false);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("plugin:opener|open_url", { url: "https://www.youtube.com/watch?v=preview-private" }));
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("shows an opener failure and lets the user try again", async () => {
    vi.mocked(invoke).mockRejectedValueOnce("系统浏览器无法启动");
    const view = render(<YouTubeUploadJobs model={completed} onRevealPath={vi.fn()} />);
    fireEvent.click(view.getByRole("link", { name: "打开 YouTube 视频" }));
    await waitFor(() => expect(view.getByRole("alert").textContent).toContain("系统浏览器无法启动"));
    fireEvent.click(view.getByRole("link", { name: "打开 YouTube 视频" }));
    await waitFor(() => expect(view.queryByRole("alert")).toBeNull());
    expect(invoke).toHaveBeenCalledTimes(2);
  });
});


it("shows concurrent uploads and lets a waiting job be cancelled independently", async () => {
  const cancel = vi.fn().mockResolvedValue(undefined);
  const jobs = ["uploading", "uploading", "uploading", "queued"].map((status, i) => ({
    ...completed.jobs[0], id: `job-${i}`, title: `视频 ${i}`, youtubeUrl: null,
    status: status as "uploading" | "queued",
  }));
  const view = render(<YouTubeUploadJobs model={{ ...completed, jobs, cancel }} onRevealPath={vi.fn()} />);
  expect(view.getByRole("status").textContent).toBe("最多同时上传 5 个 · 正在处理 3 个 · 排队 1 个");
  fireEvent.click(within(view.getAllByTestId("youtube-job-row")[3]).getByRole("button", { name: "取消" }));
  await waitFor(() => expect(cancel).toHaveBeenCalledExactlyOnceWith("job-3"));
});


it("offers pause, continue and retry on the appropriate jobs", async () => {
  const pause = vi.fn().mockResolvedValue(undefined);
  const resume = vi.fn().mockResolvedValue(undefined);
  const retry = vi.fn().mockResolvedValue(undefined);
  const jobs = (["uploading", "paused", "failed", "pausing"] as const).map((status, i) => ({
    ...completed.jobs[0], id: `control-${i}`, status, youtubeUrl: null,
  }));
  const view = render(<YouTubeUploadJobs model={{ ...completed, jobs, pause, resume, retry }} onRevealPath={vi.fn()} />);
  const rows = view.getAllByTestId("youtube-job-row");
  fireEvent.click(within(rows[0]).getByRole("button", { name: "暂停" }));
  fireEvent.click(within(rows[1]).getByRole("button", { name: "继续" }));
  fireEvent.click(within(rows[2]).getByRole("button", { name: "重试" }));
  await waitFor(() => {
    expect(pause).toHaveBeenCalledExactlyOnceWith("control-0");
    expect(resume).toHaveBeenCalledExactlyOnceWith("control-1");
    expect(retry).toHaveBeenCalledExactlyOnceWith("control-2");
  });
  expect(within(rows[3]).getByText("正在暂停")).toBeTruthy();
  expect(within(rows[3]).queryByRole("button", { name: "继续" })).toBeNull();
});

it("deletes an upload record by its stable job id", async () => {
  const removeJob = vi.fn().mockResolvedValue(undefined);
  const view = render(<YouTubeUploadJobs model={{ ...completed, removeJob }} onRevealPath={vi.fn()} />);

  fireEvent.click(view.getByRole("button", { name: "删除" }));

  await waitFor(() => expect(removeJob).toHaveBeenCalledExactlyOnceWith("preview-upload-private"));
});
