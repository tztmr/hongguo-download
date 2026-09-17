import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { DesktopRuntime } from "./DesktopRuntime";
import App from "./App";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn().mockRejectedValue(new Error("unexpected native call")), isTauri: () => false }));
function bridge(value?: unknown) { Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, writable: true, value }); }
afterEach(() => { cleanup(); bridge(); vi.useRealTimers(); vi.mocked(invoke).mockClear(); vi.unstubAllGlobals(); window.history.replaceState({}, "", "/"); });
describe("desktop startup boundary", () => {
  it("does not mount native features in an ordinary browser or misreport three source failures", () => {
    window.history.replaceState({}, "", "/"); bridge();
    render(<DesktopRuntime><App /></DesktopRuntime>);
    expect(screen.getByRole("heading", { name: "请从桌面应用打开红果下载" })).toBeTruthy();
    expect(screen.getByRole("link", { name: "进入界面演示" }).getAttribute("href")).toBe("?preview=library");
    expect(screen.queryByText(/暂不可用|reading 'invoke'/)).toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });
  it("waits for the callable bridge instead of accepting a native marker alone, then recovers automatically", async () => {
    window.history.replaceState({}, "", "/"); vi.stubGlobal("isTauri", true); bridge({}); vi.useFakeTimers();
    render(<DesktopRuntime><p>桌面应用内容</p></DesktopRuntime>);
    expect(screen.getByText("正在连接桌面功能…")).toBeTruthy();
    expect(screen.queryByText("桌面应用内容")).toBeNull();
    bridge({ invoke: vi.fn() });
    await act(async () => { vi.advanceTimersByTime(100); });
    expect(screen.getByText("桌面应用内容")).toBeTruthy();
  });
  it("bounds initialization waiting and lets a later bridge recover on retry", async () => {
    window.history.replaceState({}, "", "/"); vi.stubGlobal("isTauri", true); bridge(); vi.useFakeTimers();
    render(<DesktopRuntime><p>已连接</p></DesktopRuntime>);
    await act(async () => { vi.advanceTimersByTime(5000); });
    expect(screen.getByText("桌面功能尚未就绪")).toBeTruthy();
    bridge({ invoke: vi.fn() }); fireEvent.click(screen.getByRole("button", { name: "重新检测连接" }));
    expect(screen.getByText("已连接")).toBeTruthy();
  });
  it("renders an explicitly labeled preview and keeps refresh, category browsing and all ranks away from native IPC", async () => {
    window.history.replaceState({}, "", "/?preview=library"); bridge();
    render(<DesktopRuntime><App /></DesktopRuntime>);
    expect(screen.getByText(/界面演示 · 剧目与频道均为示例数据/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "重新加载推荐" }));
    const contentType = () => screen.getByRole("generic", { name: "内容类型" });
    fireEvent.click(within(contentType()).getByRole("button", { name: "AI剧" }));
    await screen.findByText(/AI剧推荐 · 已加载 40 部/);
    fireEvent.click(within(contentType()).getByRole("button", { name: "真人剧" }));
    fireEvent.click(screen.getByRole("button", { name: "分类浏览" }));
    await screen.findByText(/共 40 部/);
    fireEvent.click(screen.getByRole("button", { name: "榜单" }));
    for (const name of ["真人剧", "漫剧", "AI剧"]) {
      fireEvent.click(within(contentType()).getByRole("button", { name }));
      await waitFor(() => expect(screen.queryByText(/正在加载.*剧/)).toBeNull());
    }
    expect(screen.queryByText(/暂不可用|reading 'invoke'|unexpected native call/)).toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });
});
