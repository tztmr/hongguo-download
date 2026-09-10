import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { open } from "@tauri-apps/plugin-dialog";
import { previewYouTubeModel } from "../preview";
import { YouTubeSettings } from "./YouTubeSettings";
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
it("explains saved authorization separately from verified permissions", () => {
  render(<YouTubeSettings model={previewYouTubeModel} />);
  expect(screen.getByText(/实际权限以接口检查结果为准/)).toBeTruthy();
  expect(screen.getByText(/只读统计权限/)).toBeTruthy();
  fireEvent.click(screen.getByText("授权或统计失败怎么办？"));
  expect(screen.getByRole("link", { name: "YouTube Analytics API" })).toBeTruthy();
});
it("shows credential selection errors instead of dropping the rejected promise", async () => {
  vi.mocked(open).mockRejectedValue(new Error("无法打开凭证文件"));
  render(<YouTubeSettings model={previewYouTubeModel} />);
  fireEvent.click(screen.getByRole("button", { name: "导入凭证" }));
  expect((await screen.findByRole("alert")).textContent).toContain("无法打开凭证文件");
});
it("keeps authorization pending and prevents duplicate authorization", async () => {
  let resolve!: () => void;
  const authorize = vi.fn(() => new Promise<void>(done => { resolve = done; }));
  render(<YouTubeSettings model={{ ...previewYouTubeModel, authorize }} />);
  fireEvent.click(screen.getByRole("button", { name: "重新授权 / 补充权限" }));
  expect(screen.getByRole("status").textContent).toContain("等待回调");
  expect((screen.getByRole("button", { name: "授权中…" }) as HTMLButtonElement).disabled).toBe(true);
  expect(authorize).toHaveBeenCalledTimes(1);
  await act(async () => resolve());
});
