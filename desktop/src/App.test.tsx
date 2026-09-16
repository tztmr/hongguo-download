import { fireEvent, render, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import App from "./App";

vi.mock("./youtube/managementCommands", () => ({ managementCommands: { list: vi.fn().mockResolvedValue({ items: [], nextPageToken: null }) } }));

describe("App preview workflow", () => {
  it("saves CPU execution from automation and shares that choice with global settings", async () => {
    window.history.replaceState({}, "", "/?preview=automation");
    const view = render(<App />);
    fireEvent.click(await view.findByRole("button", { name: /媒体处理/ }));
    fireEvent.change(view.getByRole("combobox", { name: "AI 计算设备" }), { target: { value: "cpu" } });
    await waitFor(() => expect(view.getByRole("combobox", { name: "AI 计算设备" })).toHaveProperty("value", "cpu"));
    fireEvent.click(view.getByRole("button", { name: "设置" }));
    expect(await view.findByRole("radio", { name: /仅使用 CPU/ })).toHaveProperty("checked", true);
  });
  it("retains an unsaved network draft across sidebar navigation", async () => {
    window.history.replaceState({}, "", "/?preview=library");
    const view = render(<App />);
    fireEvent.click(view.getByRole("button", { name: "设置" }));
    fireEvent.change(await view.findByRole("textbox", { name: "代理地址" }), { target: { value: "http://127.0.0.1:7890" } });
    fireEvent.click(view.getByRole("button", { name: "首页" }));
    fireEvent.click(view.getByRole("button", { name: "设置" }));
    expect((view.getByRole("textbox", { name: "代理地址" }) as HTMLInputElement).value).toBe("http://127.0.0.1:7890");
  });

  it("retains an unsaved automation draft across sidebar navigation", async () => {
    window.history.replaceState({}, "", "/?preview=automation");
    const view = render(<App />);
    const input = await view.findByRole("spinbutton", { name: /最多集数/ });
    fireEvent.change(input, { target: { value: "123" } });
    fireEvent.click(view.getByRole("button", { name: "设置" }));
    expect(view.queryByRole("heading", { name: "24 小时自动追剧" })).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "自动追剧" }));
    expect((view.getByRole("spinbutton", { name: /最多集数/ }) as HTMLInputElement).value).toBe("123");
    expect(view.getAllByText(/有未保存/).length).toBeGreaterThan(0);
  });
  it("opens video management and analytics from the sidebar instead of download tabs", async () => {
    window.history.replaceState({}, "", "/?preview=downloads");
    const view = render(<App />);
    expect(view.queryByRole("tab", { name: /Youtube管理|平台视频管理/ })).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "视频管理" }));
    expect(view.getByRole("heading", { name: "视频管理" })).toBeTruthy();
    expect(view.getByRole("button", { name: "视频管理" }).getAttribute("aria-current")).toBe("page");
    expect(view.queryByRole("heading", { name: "下载管理" })).toBeNull();
    // This sidebar page is loaded lazily; allow module loading under a full CI run.
    await view.findByText("已加载全部频道视频与 Shorts", {}, { timeout: 5000 });
    expect(view.getByRole("button", { name: "一键删除封锁视频" })).toHaveProperty("disabled", false);
    expect(view.getByRole("button", { name: "一键设为私人" })).toHaveProperty("disabled", false);
    fireEvent.click(view.getByRole("button", { name: "数据分析" }));
    expect(view.getByRole("heading", { name: "数据分析" })).toBeTruthy();
    expect(await view.findByText("频道累计观看次数")).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "设置" }));
    expect(view.getByRole("heading", { name: "设置" })).toBeTruthy();
    expect(view.queryByRole("heading", { name: "视频管理" })).toBeNull();
  });
  it("retains video filters and analytics range when navigating away and back", async () => {
    window.history.replaceState({}, "", "/?preview=downloads");
    const view = render(<App />);
    fireEvent.click(view.getByRole("button", { name: "视频管理" }));
    await view.findByText("已加载全部频道视频与 Shorts");
    const search = view.getByRole("searchbox", { name: "搜索频道视频" });
    fireEvent.change(search, { target: { value: "保留筛选" } });
    fireEvent.click(view.getByRole("button", { name: "数据分析" }));
    fireEvent.click(await view.findByRole("button", { name: "7 天" }));
    fireEvent.click(view.getByRole("button", { name: "视频管理" }));
    expect(view.getByRole("searchbox", { name: "搜索频道视频" })).toBe(search);
    expect((search as HTMLInputElement).value).toBe("保留筛选");
    fireEvent.click(view.getByRole("button", { name: "数据分析" }));
    expect(view.getByRole("button", { name: "7 天" }).className).toContain("active");
  });
  it("preserves the chosen download and filter after leaving and returning to download management", async () => {
    window.history.replaceState({}, "", "/?preview=library");
    const view = render(<App />);
    fireEvent.click(view.getByRole("button", { name: /下载管理/ }));
    const rows = await view.findAllByTestId("download-batch-row");
    fireEvent.click(within(rows[1]).getByRole("button", { name: /任务详情/ }));
    const selectedHeading = within(view.getByRole("complementary", { name: "任务详情" })).getAllByRole("heading")[1].textContent;
    fireEvent.change(view.getByRole("searchbox", { name: "搜索下载任务" }), {target:{value: selectedHeading}});
    fireEvent.click(view.getByRole("button", { name: "首页" }));
    expect(view.queryByRole("heading", { name: "下载管理" })).toBeNull();
    fireEvent.click(view.getByRole("button", { name: /下载管理/ }));
    expect(view.getByRole("searchbox", { name: "搜索下载任务" }).getAttribute("value")).toBe(selectedHeading);
    expect(within(view.getByRole("complementary", { name: "任务详情" })).getAllByRole("heading")[1].textContent).toBe(selectedHeading);
  });

  it("opens monitor details in place and retains the list and scroll after closing", async () => {
    window.history.replaceState({}, "", "/?preview=library");
    const view = render(<App />);
    fireEvent.click(view.getByRole("button", { name: /新剧监听/ }));
    await waitFor(() => expect(view.container.querySelectorAll(".monitor-card")).toHaveLength(20));
    fireEvent.click(view.getByRole("button", { name: "AI剧" }));
    await waitFor(() => expect(view.container.querySelectorAll(".monitor-card")).toHaveLength(20));
    const scroller = view.getByTestId("monitor-scroll");
    scroller.scrollTop = 360;
    fireEvent.click(view.container.querySelectorAll(".monitor-card")[1]);
    const dialog = view.getByRole("dialog", { name: "新剧详情" });
    expect(view.queryByRole("heading", { name: "首页推荐" })).toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: /加入下载队列/ }));
    expect(within(dialog).getByText(/已加入 1 集/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "关闭详情" }));
    expect(view.queryByRole("dialog")).toBeNull();
    expect(view.getByTestId("monitor-scroll")).toBe(scroller);
    expect(scroller.scrollTop).toBe(360);
    expect(view.getByRole("button", { name: "AI剧" }).className).toContain("active");
  });

  it("selects an episode range, enqueues it and opens grouped download management", async () => {
    window.history.replaceState({}, "", "/?preview=library");
    const view = render(<App />);

    await waitFor(() => expect(view.getAllByText("天下第一纨绔").length).toBeGreaterThan(0));
    fireEvent.change(view.getByLabelText("起始集"), { target: { value: "1" } });
    fireEvent.change(view.getByLabelText("结束集"), { target: { value: "3" } });
    fireEvent.click(view.getByRole("button", { name: "选择范围" }));
    fireEvent.click(view.getByRole("button", { name: "加入下载队列（已选 3 集）" }));

    expect(view.getByText("已加入 3 集，跳过 0 个重复项")).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: /下载管理/ }));
    expect(view.getByRole("heading", { name: "下载管理" })).toBeTruthy();
    expect(await view.findAllByTestId("download-batch-row")).toHaveLength(4);
  });

  it("opens the five-column monitor and applies a 720p setting to the inspector", async () => {
    window.history.replaceState({}, "", "/?preview=library");
    const view = render(<App />);
    await waitFor(() => expect(view.getAllByText("天下第一纨绔").length).toBeGreaterThan(0));

    fireEvent.click(view.getByRole("button", { name: /新剧监听/ }));
    await waitFor(() => expect(view.container.querySelectorAll(".monitor-card")).toHaveLength(20));
    expect(view.getByRole("heading", { name: "新剧监听" })).toBeTruthy();

    fireEvent.click(view.getByRole("button", { name: "设置" }));
    await waitFor(() => expect(view.getByRole("heading", { name: "设置" })).toBeTruthy());
    fireEvent.click(view.getByRole("radio", { name: /720p/ }));
    fireEvent.click(view.getByRole("button", { name: "首页" }));
    await waitFor(() => expect(view.getByText(/720p/, { selector: ".inspector-copy p" })).toBeTruthy());
  });
});
