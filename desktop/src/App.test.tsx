import { fireEvent, render, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import App from "./App";

describe("App preview workflow", () => {
  it("preserves the chosen download and filter after leaving and returning to download management", async () => {
    window.history.replaceState({}, "", "/?preview=library");
    const view = render(<App />);
    fireEvent.click(view.getByRole("button", { name: /下载管理/ }));
    const rows = view.getAllByTestId("download-batch-row");
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
    expect(view.getAllByTestId("download-batch-row")).toHaveLength(4);
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
    await waitFor(() => expect(view.getByText(/720p/)).toBeTruthy());
  });
});
