import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { DownloadBatch } from "../download/model";
import { YouTubeUploadDialog } from "./YouTubeUploadDialog";

const batch: DownloadBatch = {
  id: "batch-ai", bookId: "book-ai", title: "AI 漫剧", cover: "", paused: false, createdAt: 1, updatedAt: 2,
  series: { bookId: "book-ai", seriesId: "series-ai", title: "AI 漫剧", cover: "", abstract: "剧情简介", category: "AI漫剧", contentTypeCode: 2 },
  items: [{ id: "episode", itemId: "source", episodeIndex: 1, episodeTitle: "第1集", definition: "1080p", status: "done", percent: 100, received: 1, path: "/Downloads/e1.mp4", addedAt: 1 }],
};

describe("YouTubeUploadDialog", () => {
  it("defaults to film and animation, synthetic content, and all three confirmations", () => {
    const submit = vi.fn();
    const ordinaryBatch = {
      ...batch,
      title: "普通短剧",
      series: { ...batch.series, title: "普通短剧", category: "真人剧" },
    };
    const view = render(<YouTubeUploadDialog batch={ordinaryBatch} sourcePath="/Downloads/merged.mp4" onClose={vi.fn()} onSubmit={submit} />);
    const button = view.getByRole("button", { name: "确认上传" }) as HTMLButtonElement;
    expect((view.getByLabelText("YouTube 类别") as HTMLSelectElement).value).toBe("1");
    expect((view.getByLabelText("合成内容") as HTMLSelectElement).value).toBe("yes");
    expect(view.getAllByRole("checkbox").every((checkbox) => (checkbox as HTMLInputElement).checked)).toBe(true);
    expect(button.disabled).toBe(false);
    fireEvent.click(button);
    expect(submit).toHaveBeenCalledWith(expect.objectContaining({
      filePath: "/Downloads/merged.mp4", categoryId: "1", privacyStatus: "private", containsSyntheticMedia: true,
      audienceConfirmed: true, syntheticMediaConfirmed: true, publishConfirmed: true,
    }));
  });

  it("uses the drama category labels as separate YouTube tags", () => {
    const submit = vi.fn();
    const categorizedBatch = {
      ...batch,
      series: {
        ...batch.series,
        category: "真人剧",
        categoryTags: ["都市", "逆袭", "甜宠"],
      },
    };

    const view = render(<YouTubeUploadDialog batch={categorizedBatch} sourcePath="/Downloads/merged.mp4" onClose={vi.fn()} onSubmit={submit} />);

    expect((view.getByLabelText("YouTube 标签") as HTMLInputElement).value).toBe("都市, 逆袭, 甜宠");
    fireEvent.click(view.getByRole("button", { name: "确认上传" }));
    expect(submit).toHaveBeenCalledWith(expect.objectContaining({ tags: ["都市", "逆袭", "甜宠"] }));
  });
});
