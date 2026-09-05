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
  it("requires all three explicit confirmations and defaults AI titles to synthetic content", () => {
    const submit = vi.fn();
    const view = render(<YouTubeUploadDialog batch={batch} sourcePath="/Downloads/merged.mp4" onClose={vi.fn()} onSubmit={submit} />);
    const button = view.getByRole("button", { name: "确认上传" }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    expect((view.getByLabelText("合成内容") as HTMLSelectElement).value).toBe("yes");
    for (const checkbox of view.getAllByRole("checkbox")) fireEvent.click(checkbox);
    expect(button.disabled).toBe(false);
    fireEvent.click(button);
    expect(submit).toHaveBeenCalledWith(expect.objectContaining({
      filePath: "/Downloads/merged.mp4", privacyStatus: "private", containsSyntheticMedia: true,
      audienceConfirmed: true, syntheticMediaConfirmed: true, publishConfirmed: true,
    }));
  });
});
