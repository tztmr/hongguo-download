import { act, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { deferPage } from "./DeferredPage";

function Content({ hidden = false }: { hidden?: boolean }) { return <main hidden={hidden}><input aria-label="页面草稿" defaultValue="" /></main>; }
afterEach(() => vi.restoreAllMocks());

describe("deferred pages", () => {
  it("loads on first visit, hides loading feedback offscreen and retains the mounted draft", async () => {
    let complete!: (value: { default: typeof Content }) => void;
    const load = vi.fn(() => new Promise<{ default: typeof Content }>(resolve => { complete = resolve; }));
    const Page = deferPage("测试页面", load);
    expect(load).not.toHaveBeenCalled();
    const view = render(<Page />);
    expect(view.getByRole("status").textContent).toContain("正在加载测试页面");
    view.rerender(<Page hidden />);
    expect(view.queryByRole("status")).toBeNull();
    await act(async () => complete({ default: Content }));
    view.rerender(<Page />);
    fireEvent.change(view.getByRole("textbox"), { target: { value: "保留草稿" } });
    view.rerender(<Page hidden />);
    expect(view.queryByRole("textbox")).toBeNull();
    view.rerender(<Page />);
    expect(view.getByRole("textbox")).toHaveProperty("value", "保留草稿");
    expect(load).toHaveBeenCalledTimes(1);
  });

  it("retries a failed page import without reloading the application", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const load = vi.fn().mockRejectedValueOnce(new Error("chunk unavailable")).mockResolvedValue({ default: Content });
    const Page = deferPage("测试页面", load);
    const view = render(<><nav>主导航仍可操作</nav><Page /></>);
    expect((await view.findByRole("alert")).textContent).toContain("页面加载失败");
    expect(view.getByRole("navigation").textContent).toBe("主导航仍可操作");
    fireEvent.click(view.getByRole("button", { name: "重新加载页面" }));
    await view.findByRole("textbox");
    expect(load).toHaveBeenCalledTimes(2);
  });
});
