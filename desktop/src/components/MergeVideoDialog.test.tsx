import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { DownloadBatch } from "../download/model";
import { MergeVideoDialog } from "./MergeVideoDialog";

const batchWithOutOfOrderItems: DownloadBatch = {
  id: "batch-a",
  bookId: "book-a",
  title: "天下第一纨绔",
  cover: "",
  series: {
    bookId: "book-a",
    seriesId: "book-a",
    title: "天下第一纨绔",
    cover: "",
    abstract: "",
    category: "",
    contentTypeCode: 1,
  },
  paused: false,
  createdAt: 1,
  updatedAt: 2,
  items: [
    { id: "a2", itemId: "e2", episodeIndex: 2, episodeTitle: "第 2 集", definition: "1080p", status: "done", percent: 100, received: 2, path: "/Downloads/e2.mp4", addedAt: 1 },
    { id: "a1", itemId: "e1", episodeIndex: 1, episodeTitle: "第 1 集", definition: "1080p", status: "done", percent: 100, received: 1, path: "/Downloads/e1.mp4", addedAt: 1 },
  ],
};

const incompleteBatch: DownloadBatch = {
  ...batchWithOutOfOrderItems,
  items: [
    batchWithOutOfOrderItems.items[0],
    { ...batchWithOutOfOrderItems.items[1], status: "running", percent: 40, path: undefined },
  ],
};

describe("MergeVideoDialog", () => {
  it("defaults to automatic lossless merge and submits episode-sorted completed paths", async () => {
    const submit = vi.fn();
    const view = render(
      <MergeVideoDialog batch={batchWithOutOfOrderItems} onSubmit={submit} onClose={vi.fn()} />,
    );
    expect((view.getByRole("combobox", { name: "合并方式" }) as HTMLSelectElement).value).toBe("auto");
    expect(view.getByText("天下第一纨绔")).toBeTruthy();
    expect(view.getByLabelText("合并剧集").textContent).toContain("第 1 集");
    expect(view.getByLabelText("合并剧集").textContent).toContain("第 2 集");
    fireEvent.click(view.getByRole("button", { name: "开始合并" }));
    expect(submit).toHaveBeenCalledWith(
      expect.objectContaining({
        mode: "auto",
        quality: "high",
        conflictPolicy: "failIfExists",
        outputName: "天下第一纨绔.mp4",
        inputs: [
          expect.objectContaining({ episodeIndex: 1, path: "/Downloads/e1.mp4" }),
          expect.objectContaining({ episodeIndex: 2, path: "/Downloads/e2.mp4" }),
        ],
      }),
    );
  });

  it("submits selected transcode quality and disables quality for lossless mode", () => {
    const submit = vi.fn();
    const view = render(<MergeVideoDialog batch={batchWithOutOfOrderItems} onSubmit={submit} onClose={vi.fn()} />);
    fireEvent.change(view.getByRole("combobox", { name: "合并方式" }), { target: { value: "copy" } });
    expect((view.getByRole("combobox", { name: "转码画质" }) as HTMLSelectElement).disabled).toBe(true);
    fireEvent.change(view.getByRole("combobox", { name: "合并方式" }), { target: { value: "transcode" } });
    fireEvent.change(view.getByRole("combobox", { name: "转码画质" }), { target: { value: "compact" } });
    fireEvent.click(view.getByRole("button", { name: "开始合并" }));
    expect(submit).toHaveBeenCalledWith(expect.objectContaining({ mode: "transcode", quality: "compact", conflictPolicy: "failIfExists" }));
  });

  it("keeps incomplete batches from submitting and never offers overwrite", () => {
    const submit = vi.fn();
    const view = render(<MergeVideoDialog batch={incompleteBatch} onSubmit={submit} onClose={vi.fn()} />);
    expect((view.getByRole("button", { name: "开始合并" }) as HTMLButtonElement).disabled).toBe(true);
    expect(view.queryByRole("radio", { name: "覆盖已有文件" })).toBeNull();
    expect(view.queryByRole("radio", { name: "不覆盖已有文件" })).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "开始合并" }));
    expect(submit).not.toHaveBeenCalled();
  });
});
