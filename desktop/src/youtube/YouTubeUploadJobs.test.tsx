import { act, fireEvent, render, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { previewYouTubeModel } from "../preview";
import { YouTubeUploadJobs } from "./YouTubeUploadJobs";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: vi.fn() }));
const completed = { ...previewYouTubeModel, jobs: [previewYouTubeModel.jobs[1]] };

it("bulk starts paused and failed visible selections and skips completed jobs", async () => {
  const resume = vi.fn().mockResolvedValue(undefined);
  const retry = vi.fn().mockResolvedValue(undefined);
  const jobs = (["paused", "failed", "cancelled", "completed"] as const).map((status, i) => ({
    ...completed.jobs[0], id: `bulk-${i}`, title: `视频${i}`, status,
  }));
  const view = render(<YouTubeUploadJobs model={{ ...completed, jobs, resume, retry }} onRevealPath={vi.fn()} />);
  fireEvent.click(view.getByRole("checkbox", { name: "全选当前可见上传任务" }));
  fireEvent.click(view.getByRole("button", { name: /^批量开始/ }));
  await waitFor(() => expect(retry).toHaveBeenCalledTimes(2));
  expect(resume).toHaveBeenCalledExactlyOnceWith("bulk-0");
  expect(retry.mock.calls).toEqual([["bulk-1"], ["bulk-2"]]);
});

it("bulk pause captures selected visible pausable jobs and reports individual failures", async () => {
  const pause = vi.fn().mockImplementation((id) => id === "pause-1" ? Promise.reject({ message: "暂停失败" }) : Promise.resolve());
  const jobs = (["uploading", "queued", "paused", "processing"] as const).map((status, i) => ({
    ...completed.jobs[0], id: `pause-${i}`, title: `视频${i}`, status,
  }));
  const view = render(<YouTubeUploadJobs model={{ ...completed, jobs, pause }} onRevealPath={vi.fn()} />);
  fireEvent.click(view.getByRole("checkbox", { name: "全选当前可见上传任务" }));
  fireEvent.click(view.getByRole("button", { name: /^批量暂停/ }));
  await waitFor(() => expect(view.getByRole("alert").textContent).toContain("1 项暂停失败"));
  expect(pause.mock.calls).toEqual([["pause-0"], ["pause-1"]]);
});

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


describe("upload list selection", () => {
  const jobs = [
    { ...completed.jobs[0], id: "alpha", title: "Alpha" },
    { ...completed.jobs[0], id: "beta", title: "Beta", status: "failed" as const },
  ];
  const checkbox = (view: ReturnType<typeof render>, name: string) => view.getByRole("checkbox", { name }) as HTMLInputElement;
  const bulkButton = (view: ReturnType<typeof render>) => view.getByRole("button", { name: "批量删除" }) as HTMLButtonElement;

  it("retains hidden selection while deleting only selected visible records and preserving filters", async () => {
    const removeJob = vi.fn().mockResolvedValue(undefined);
    const onNotice = vi.fn();
    const model = { ...completed, jobs, removeJob };
    const view = render(<YouTubeUploadJobs model={model} onRevealPath={vi.fn()} onNotice={onNotice} />);
    expect(bulkButton(view).disabled).toBe(true);
    fireEvent.click(checkbox(view, "全选当前可见上传任务"));
    fireEvent.click(within(view.getByRole("group", { name: "上传状态筛选" })).getByRole("button", { name: /已完成/ }));
    fireEvent.change(view.getByRole("searchbox"), { target: { value: "Alpha" } });
    expect(view.getByText("已选 1 项（当前可见），共选 2 项")).toBeTruthy();
    fireEvent.click(bulkButton(view));
    await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.stringContaining("已删除 1 项")));
    expect(removeJob).toHaveBeenCalledExactlyOnceWith("alpha");
    expect((view.getByRole("searchbox") as HTMLInputElement).value).toBe("Alpha");
    expect(view.getByText("已选 0 项（当前可见），共选 1 项")).toBeTruthy();
    fireEvent.change(view.getByRole("searchbox"), { target: { value: "" } });
    fireEvent.click(within(view.getByRole("group", { name: "上传状态筛选" })).getByRole("button", { name: /全部/ }));
    expect(checkbox(view, "选择上传任务：Beta").checked).toBe(true);
    expect(checkbox(view, "选择上传任务：Alpha").checked).toBe(false);
  });

  it("selects and clears only visible rows, handles mixed/empty lists and prunes removed ids", () => {
    const model = { ...completed, jobs };
    const view = render(<YouTubeUploadJobs model={model} onRevealPath={vi.fn()} />);
    fireEvent.click(checkbox(view, "选择上传任务：Alpha"));
    expect(checkbox(view, "全选当前可见上传任务").indeterminate).toBe(true);
    fireEvent.change(view.getByRole("searchbox"), { target: { value: "Beta" } });
    fireEvent.click(checkbox(view, "全选当前可见上传任务"));
    fireEvent.click(checkbox(view, "全选当前可见上传任务"));
    expect(view.getByText("已选 0 项（当前可见），共选 1 项")).toBeTruthy();
    fireEvent.change(view.getByRole("searchbox"), { target: { value: "missing" } });
    expect(checkbox(view, "全选当前可见上传任务").disabled).toBe(true);
    expect(bulkButton(view).disabled).toBe(true);
    view.rerender(<YouTubeUploadJobs model={{ ...model, jobs: [jobs[1]] }} onRevealPath={vi.fn()} />);
    expect(view.getByText("已选 0 项（当前可见），共选 0 项")).toBeTruthy();
  });

  it("guards overlapping deletes, deselects successes, and retains failures for retry", async () => {
    let finish!: () => void;
    const removeJob = vi.fn().mockImplementation((id: string) => id === "alpha"
      ? new Promise<void>((resolve) => { finish = resolve; }) : Promise.reject({ message: "无法删除记录" }));
    const onNotice = vi.fn();
    const view = render(<YouTubeUploadJobs model={{ ...completed, jobs, removeJob }} onRevealPath={vi.fn()} onNotice={onNotice} />);
    fireEvent.click(checkbox(view, "全选当前可见上传任务"));
    fireEvent.click(bulkButton(view));
    fireEvent.click(bulkButton(view));
    expect(bulkButton(view).disabled).toBe(true);
    for (const row of view.getAllByTestId("youtube-job-row")) {
      const button = within(row).getByRole("button", { name: "删除" }) as HTMLButtonElement;
      expect(button.disabled).toBe(true);
      fireEvent.click(button);
    }
    await act(async () => finish());
    await waitFor(() => expect(bulkButton(view).disabled).toBe(false));
    expect(removeJob.mock.calls).toEqual([["alpha"], ["beta"]]);
    expect(checkbox(view, "选择上传任务：Alpha").checked).toBe(false);
    expect(checkbox(view, "选择上传任务：Beta").checked).toBe(true);
    expect(view.getByRole("alert").textContent).toContain("1 项删除失败");
    expect(view.getByRole("alert").textContent).toContain("无法删除记录");
    expect(onNotice).toHaveBeenCalledWith(expect.stringContaining("已删除 1 项"));
    removeJob.mockResolvedValue(undefined);
    fireEvent.click(bulkButton(view));
    await waitFor(() => expect(view.getByText("已选 0 项（当前可见），共选 0 项")).toBeTruthy());
    expect(removeJob.mock.calls).toEqual([["alpha"], ["beta"], ["beta"]]);
  });

  it("blocks bulk deletion during a selected row action and notifies single deletion", async () => {
    let finish!: () => void;
    const pause = vi.fn(() => new Promise<void>((resolve) => { finish = resolve; }));
    const removeJob = vi.fn().mockResolvedValue(undefined);
    const onNotice = vi.fn();
    const view = render(<YouTubeUploadJobs model={{ ...completed, jobs: [{ ...jobs[0], status: "uploading" }], pause, removeJob }} onRevealPath={vi.fn()} onNotice={onNotice} />);
    fireEvent.click(checkbox(view, "选择上传任务：Alpha"));
    fireEvent.click(view.getByRole("button", { name: "暂停" }));
    expect(bulkButton(view).disabled).toBe(true);
    await act(async () => finish());
    fireEvent.click(view.getByRole("button", { name: "删除" }));
    await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.stringContaining("已删除 1 项")));
    expect(checkbox(view, "选择上传任务：Alpha").checked).toBe(false);
    expect(removeJob).toHaveBeenCalledExactlyOnceWith("alpha");
  });
});
