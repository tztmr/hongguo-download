import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { AutomationJobs } from "./AutomationJobs";
import type { AutomationJob } from "./automationRuntime";
afterEach(cleanup);
const job = (id: number): AutomationJob => ({ id: String(id), title: `剧目${id}`, bookId: `book-${id}`, stage: "merge", status: "pending", message: "正在合并", episodeDone: 10, episodeTotal: 10, progress: 5, updatedAt: 1000 - id });
const board = (jobs: AutomationJob[]) => <AutomationJobs jobs={jobs} loaded pending={false} onAction={vi.fn()} />;
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
