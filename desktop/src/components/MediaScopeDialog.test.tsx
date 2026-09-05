import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { MediaScopeDialog } from "./MediaScopeDialog";

describe("MediaScopeDialog", () => {
  it("only offers merged scope when a validated merged video exists", () => {
    const onSubmit = vi.fn();
    const view = render(
      <MediaScopeDialog kind="subtitleExtraction" hasMergedVideo={false} onSubmit={onSubmit} onClose={vi.fn()} />,
    );
    expect(view.queryByRole("radio", { name: "合并视频" })).toBeNull();
    expect(view.getByText(/只能按逐集处理/)).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "开始提取" }));
    expect(onSubmit).toHaveBeenCalledWith("episodes");
  });

  it("shows independent audio disclaimer and returns merged scope", () => {
    const onSubmit = vi.fn();
    const view = render(
      <MediaScopeDialog kind="audioSeparation" hasMergedVideo onSubmit={onSubmit} onClose={vi.fn()} />,
    );
    expect(view.getByText(/不保证规避 Content ID/)).toBeTruthy();
    fireEvent.click(view.getByRole("radio", { name: "合并视频" }));
    fireEvent.click(view.getByRole("button", { name: "开始分离" }));
    expect(onSubmit).toHaveBeenCalledWith("merged");
  });
});
