import { fireEvent, render, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { CategoryGroup } from "../types";
import { CategoryFilter } from "./CategoryFilter";
const groups: CategoryGroup[] = [
  { id: "background", name: "背景", items: [{ id: "", name: "全部" }, { id: "cate_757", name: "现代" }] },
  { id: "topic", name: "主题", items: [{ id: "", name: "全部" }, ...Array.from({ length: 12 }, (_, i) => ({ id: `cate_${i}`, name: i === 0 ? "仙侠" : `主题${i}` }))] },
];
describe("CategoryFilter", () => {
  it("shows all facets together and keeps a hidden selection visible", () => {
    const onChange = vi.fn();
    const view = render(<CategoryFilter groups={groups} values={{ background: "cate_757", topic: "cate_11" }} onChange={onChange} />);
    expect(view.getByRole("button", { name: "现代" }).getAttribute("aria-pressed")).toBe("true");
    expect(view.getByRole("button", { name: "主题11" }).getAttribute("aria-pressed")).toBe("true");
    expect(view.queryByRole("button", { name: "主题9" })).toBeNull();
    fireEvent.click(view.getByRole("button", { name: /更多/ }));
    fireEvent.click(view.getByRole("button", { name: "主题9" }));
    expect(onChange).toHaveBeenCalledWith("topic", "cate_9");
    fireEvent.click(within(view.getByRole("group", { name: "背景" })).getByRole("button", { name: "全部" }));
    expect(onChange).toHaveBeenCalledWith("background", "");
  });
  it("supports the video-only manju selector in single-selection mode", () => {
    const onSelect = vi.fn();
    const view = render(<CategoryFilter groups={groups} selectedId="cate_0" onSelect={onSelect} />);
    fireEvent.click(view.getByRole("button", { name: "现代" }));
    expect(onSelect).toHaveBeenCalledWith("cate_757");
  });
});
