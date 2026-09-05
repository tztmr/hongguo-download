import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { CategoryGroup } from "../types";
import { CategoryFilter } from "./CategoryFilter";

const groups: CategoryGroup[] = [
  { id: "综合", name: "综合", items: [{ id: "all", name: "全部" }] },
  {
    id: "主题情节",
    name: "主题情节",
    items: Array.from({ length: 12 }, (_, index) => ({
      id: `cate-${index + 1}`,
      name: index === 0 ? "打脸虐渣" : index === 11 ? "都市修仙" : `题材 ${index + 1}`,
    })),
  },
];

describe("CategoryFilter", () => {
  it("switches groups, expands long rows and emits the selected category", () => {
    const onSelect = vi.fn();
    const view = render(
      <CategoryFilter groups={groups} selectedId="all" onSelect={onSelect} />,
    );

    fireEvent.click(view.getByRole("button", { name: "主题情节" }));
    expect(view.getByRole("button", { name: "打脸虐渣" })).toBeTruthy();
    expect(view.queryByRole("button", { name: "都市修仙" })).toBeNull();
    fireEvent.click(view.getByRole("button", { name: /展开/ }));
    fireEvent.click(view.getByRole("button", { name: "都市修仙" }));

    expect(onSelect).toHaveBeenCalledWith("cate-12");
  });
});
