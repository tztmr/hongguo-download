import { fireEvent, render, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import App from "./App";

describe("App preview workflow", () => {
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
