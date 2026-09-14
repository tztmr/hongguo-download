import { cleanup, fireEvent, render, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { AutomationJobs } from "./AutomationJobs";
import type { AutomationJob } from "./automationRuntime";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: vi.fn() }));
beforeEach(() => {
  vi.mocked(isTauri).mockReturnValue(true);
  vi.mocked(invoke).mockReset().mockResolvedValue(undefined);
});
afterEach(cleanup);
const job = (id: number): AutomationJob => ({ id: String(id), title: `剧目${id}`, bookId: `book-${id}`, stage: "merge", status: "pending", message: "正在合并", episodeDone: 10, episodeTotal: 10, progress: 5, updatedAt: 1000 - id });
const board = (jobs: AutomationJob[]) => <AutomationJobs jobs={jobs} loaded pending={false} onAction={vi.fn()} />;
it("opens the uploaded Short in the desktop browser and shows its corresponding main video", async () => {
  const view = render(board([{
    ...job(1), status: "completed", mainVideoUrl: "https://www.youtube.com/watch?v=main-123",
    shortVideoUrl: "https://www.youtube.com/shorts/short-456",
  }]));
  fireEvent.click(view.getByRole("button", { name: /已结束/ }));
  expect(view.getByRole("link", { name: "相关视频 / Related video ↗" }).getAttribute("href")).toBe("https://studio.youtube.com/video/short-456/edit");
  const related = within(view.getByRole("region", { name: "Shorts 关联正片" }));
  expect(related.getByRole("link", { name: "https://www.youtube.com/watch?v=main-123" }).getAttribute("href")).toBe("https://www.youtube.com/watch?v=main-123");
  expect(fireEvent.click(related.getByRole("link", { name: "相关视频 / Related video ↗" }))).toBe(false);
  await waitFor(() => expect(invoke).toHaveBeenCalledExactlyOnceWith("plugin:opener|open_url", { url: "https://studio.youtube.com/video/short-456/edit" }));
});
it("finds uploaded Shorts after cleanup and does not hide the entry when a legacy main URL is missing", () => {
  const view = render(board([
    { ...job(1), status: "completed", shortVideoUrl: "https://www.youtube.com/watch?v=short-1" },
    { ...job(2), status: "completed", mainVideoUrl: "https://www.youtube.com/watch?v=main-2" },
    { ...job(3), status: "failed", shortVideoUrl: "https://youtu.be/short-3" },
    { ...job(4), shortVideoUrl: "https://notyoutube.com/shorts/unrelated" },
  ]));
  fireEvent.click(view.getByRole("button", { name: /Shorts 关联/ }));
  expect(view.getAllByRole("article").map(card => card.getAttribute("aria-label"))).toEqual(["剧目1任务", "剧目3任务"]);
  expect(view.getAllByRole("link", { name: "相关视频 / Related video ↗" }).map(link => link.getAttribute("href"))).toEqual([
    "https://studio.youtube.com/video/short-1/edit", "https://studio.youtube.com/video/short-3/edit",
  ]);
  expect(view.getAllByText("此任务未记录有效的正片地址，请在 Studio 核对同频道正片。")).toHaveLength(2);
});
it("explains when the main and Short share an ID and reports browser launch failures", async () => {
  vi.mocked(invoke).mockRejectedValueOnce("默认浏览器不可用");
  const view = render(board([{ ...job(1), shortVideoUrl: "https://youtu.be/same-id", mainVideoUrl: "https://www.youtube.com/watch?v=same-id" }]));
  expect(view.getByText("正片与 Shorts 是同一条视频，请在 Studio 选择同频道的其他视频。")).toBeTruthy();
  fireEvent.click(view.getByRole("link", { name: "相关视频 / Related video ↗" }));
  await waitFor(() => expect(view.getByRole("alert").textContent).toContain("默认浏览器不可用"));
  expect(view.getByText(/上传成功不会自动关联正片/)).toBeTruthy();
});
it("lets every unfinished drama be skipped by stable ID including active uploads", () => {
  const onAction = vi.fn();
  const jobs: AutomationJob[] = ["pending", "working", "observing", "review", "failed"].map((status, i) => ({ ...job(i), stage: "upload", status: status as AutomationJob["status"] }));
  const view = render(<AutomationJobs jobs={jobs} loaded pending={false} onAction={onAction} />);
  const buttons = view.getAllByRole("button", { name: "跳过此任务" });
  expect(buttons).toHaveLength(5);
  fireEvent.click(buttons[1]);
  expect(onAction).toHaveBeenCalledWith("1", "skip");
  view.rerender(<AutomationJobs jobs={jobs.map(j => ({ ...j, status: "skipped" }))} loaded pending={false} onAction={onAction} />);
  fireEvent.click(view.getByRole("button", { name: /已结束/ }));
  expect(view.queryByRole("button", { name: "跳过此任务" })).toBeNull();
});
it("keeps queue order and an open task while polling changes status and timestamps", () => {
  const jobs = [job(1), job(2), job(3)];
  const view = render(board(jobs));
  const cards = view.getAllByRole("article");
  fireEvent.click(cards[0].querySelector("summary")!);
  expect(cards[0].querySelector("details")!.open).toBe(true);
  const updated: AutomationJob[] = jobs.map((j, index) => ({ ...j, status: index === 2 ? "working" : "pending", updatedAt: 2000 + index, progress: 20 + index }));
  view.rerender(board(updated));
  expect(view.getAllByRole("article").map(card => card.getAttribute("aria-label"))).toEqual(["剧目1任务", "剧目2任务", "剧目3任务"]);
  expect(view.getAllByRole("article")[0]).toBe(cards[0]);
  expect(cards[0].querySelector("details")!.open).toBe(true);
  expect(cards[0].querySelector("progress")!.value).toBe(20);
});

it("uses persisted queue order when a refreshed snapshot arrives in another order", () => {
  const jobs = [
    { ...job(3), queueOrder: 30 },
    { ...job(1), queueOrder: 10 },
    { ...job(2), queueOrder: 20 },
  ];
  const view = render(board(jobs));
  expect(view.getAllByRole("article").map(card => card.getAttribute("aria-label"))).toEqual(["剧目1任务", "剧目2任务", "剧目3任务"]);
  view.rerender(board(jobs.slice().reverse().map(item => ({ ...item, updatedAt: 3000 }))));
  expect(view.getAllByRole("article").map(card => card.getAttribute("aria-label"))).toEqual(["剧目1任务", "剧目2任务", "剧目3任务"]);
});
it("keeps the current page stable when other jobs progress and new jobs are appended", () => {
  const jobs = Array.from({ length: 25 }, (_, i) => job(i + 1));
  const view = render(board(jobs));
  fireEvent.click(view.getByRole("button", { name: "下一页" }));
  const before = view.getAllByRole("article").map(card => card.getAttribute("aria-label"));
  view.rerender(board([...jobs.map(j => ({ ...j, status: j.id === "24" ? "working" as const : "pending" as const, updatedAt: Number(j.id) * 100 })), job(26)]));
  expect(view.getAllByRole("article").slice(0, 5).map(card => card.getAttribute("aria-label"))).toEqual(before);
  expect(view.getByText("第 2 / 2 页 · 26 个任务")).toBeTruthy();
});

it("does not mistake dispatcher polling for real media execution", () => {
  const jobs: AutomationJob[] = [{ ...job(1), mediaState: "running" }, { ...job(2), mediaState: "queued", status: "working" }];
  const view = render(board(jobs));
  expect(view.getAllByRole("article")[0].querySelector(".auto-task-status")?.textContent).toBe("处理中");
  expect(view.getAllByRole("article")[1].querySelector(".auto-task-status")?.textContent).toBe("排队中");
  expect(view.getAllByRole("article")[0].className).toContain("status-working");
  expect(view.getAllByRole("article")[1].className).toContain("status-pending");
  view.rerender(board(jobs.map(j => ({ ...j, status: j.status === "working" ? "pending" : "working" }))));
  expect(view.getAllByRole("article")[0].querySelector(".auto-task-status")?.textContent).toBe("处理中");
  expect(view.getAllByRole("article")[1].querySelector(".auto-task-status")?.textContent).toBe("排队中");
  expect(view.getAllByRole("article")[0].className).toContain("status-working");
  expect(view.getAllByRole("article")[1].className).toContain("status-pending");
});
