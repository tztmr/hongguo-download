import { act, fireEvent, render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DownloadBatch } from "../download/model";
import { YouTubeUploadDialog } from "./YouTubeUploadDialog";

const checkUpload = vi.hoisted(() => vi.fn());
vi.mock("./commands", () => ({ checkYouTubeUpload: checkUpload }));
beforeEach(() => { checkUpload.mockReset().mockResolvedValue([]); });

const batch: DownloadBatch = {
  id: "batch-ai", bookId: "book-ai", title: "AI 漫剧", cover: "", paused: false, createdAt: 1, updatedAt: 2,
  series: { bookId: "book-ai", seriesId: "series-ai", title: "AI 漫剧", cover: "", abstract: "剧情简介", category: "AI漫剧", contentTypeCode: 2 },
  items: [{ id: "episode", itemId: "source", episodeIndex: 1, episodeTitle: "第1集", definition: "1080p", status: "done", percent: 100, received: 1, path: "/Downloads/e1.mp4", addedAt: 1 }],
};

describe("YouTubeUploadDialog", () => {
  it("requires an explicit video version and submits the chosen file", async () => {
    const submit = vi.fn();
    const view = render(<YouTubeUploadDialog batch={batch} sourcePath="/Downloads/merged.mp4"
      sourceOptions={[{ kind: "merged", path: "/Downloads/merged.mp4" }, { kind: "noBackgroundMusic", path: "/Downloads/clean.mp4" }]}
      channelId="channel-a" onClose={vi.fn()} onSubmit={submit} />);
    expect(view.getByRole("button", { name: "确认上传" })).toHaveProperty("disabled", true);
    expect(view.getAllByRole("radio").every((radio) => !(radio as HTMLInputElement).checked)).toBe(true);
    fireEvent.click(view.getByRole("radio", { name: "去背景音乐视频" }));
    expect(view.getByText("上传文件：/Downloads/clean.mp4")).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "确认上传" }));
    await waitFor(() => expect(submit).toHaveBeenCalledWith(expect.objectContaining({ filePath: "/Downloads/clean.mp4" })));
  });

  it("clears a previous duplicate override when a different video is chosen", async () => {
    checkUpload.mockResolvedValueOnce([{ title: "AI 漫剧", videoId: "existing", youtubeUrl: "https://www.youtube.com/watch?v=existing", reason: "sameDrama" }]);
    const submit = vi.fn();
    const view = render(<YouTubeUploadDialog batch={batch} sourcePath="/Downloads/merged.mp4"
      sourceOptions={[{ kind: "merged", path: "/Downloads/merged.mp4" }, { kind: "noBackgroundMusic", path: "/Downloads/clean.mp4" }]}
      channelId="channel-a" onClose={vi.fn()} onSubmit={submit} />);
    fireEvent.click(view.getByRole("radio", { name: "去背景音乐视频" }));
    fireEvent.click(view.getByRole("button", { name: "确认上传" }));
    await waitFor(() => expect(view.getByRole("button", { name: "仍然上传" })).toBeTruthy());
    fireEvent.click(view.getByRole("radio", { name: "合并视频（保留背景音乐）" }));
    expect(view.queryByRole("button", { name: "仍然上传" })).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "确认上传" }));
    await waitFor(() => expect(submit).toHaveBeenCalledWith(expect.objectContaining({ filePath: "/Downloads/merged.mp4", dedup: expect.objectContaining({ allowDuplicate: false }) })));
    expect(checkUpload).toHaveBeenCalledTimes(2);
  });

  it("defaults to film and animation, synthetic content, and all three confirmations", async () => {
    const submit = vi.fn();
    const ordinaryBatch = {
      ...batch,
      title: "普通短剧",
      series: { ...batch.series, title: "普通短剧", category: "真人剧" },
    };
    const view = render(<YouTubeUploadDialog batch={ordinaryBatch} sourcePath="/Downloads/merged.mp4" channelId="channel-a" onClose={vi.fn()} onSubmit={submit} />);
    const button = view.getByRole("button", { name: "确认上传" }) as HTMLButtonElement;
    expect((view.getByLabelText("YouTube 类别") as HTMLSelectElement).value).toBe("1");
    expect((view.getByLabelText("合成内容") as HTMLSelectElement).value).toBe("yes");
    expect(view.getAllByRole("checkbox").every((checkbox) => (checkbox as HTMLInputElement).checked)).toBe(true);
    expect(button.disabled).toBe(false);
    fireEvent.click(button);
    await waitFor(() => expect(submit).toHaveBeenCalledWith(expect.objectContaining({
      filePath: "/Downloads/merged.mp4", categoryId: "1", privacyStatus: "private", containsSyntheticMedia: true,
      audienceConfirmed: true, syntheticMediaConfirmed: true, publishConfirmed: true,
    })));
  });

  it("uses the drama category labels and drama title as separate YouTube tags", async () => {
    const submit = vi.fn();
    const categorizedBatch = {
      ...batch,
      series: {
        ...batch.series,
        category: "真人剧",
        categoryTags: ["都市", "逆袭", "甜宠"],
      },
    };

    const view = render(<YouTubeUploadDialog batch={categorizedBatch} sourcePath="/Downloads/merged.mp4" channelId="channel-a" onClose={vi.fn()} onSubmit={submit} />);

    expect((view.getByLabelText("YouTube 标签") as HTMLInputElement).value).toBe("都市, 逆袭, 甜宠, AI 漫剧");
    fireEvent.click(view.getByRole("button", { name: "确认上传" }));
    await waitFor(() => expect(submit).toHaveBeenCalledWith(expect.objectContaining({ tags: ["都市", "逆袭", "甜宠", "AI 漫剧"] })));
  });
});

it("stops a matching upload, shows the existing video, and requires an explicit override", async () => {
  checkUpload.mockResolvedValue([{ title: "AI 漫剧", videoId: "existing", youtubeUrl: "https://www.youtube.com/watch?v=existing", reason: "sameTitle" }]);
  const submit = vi.fn();
  const view = render(<YouTubeUploadDialog batch={batch} sourcePath="/Downloads/merged.mp4" channelId="channel-a" onClose={vi.fn()} onSubmit={submit} />);
  fireEvent.click(view.getByRole("button", { name: "确认上传" }));
  await waitFor(() => expect(view.getByRole("link", { name: "打开 YouTube 视频" }).getAttribute("href")).toBe("https://www.youtube.com/watch?v=existing"));
  expect(submit).not.toHaveBeenCalled();
  expect(view.getByRole("button", { name: "跳过本次上传" })).toBeTruthy();
  fireEvent.click(view.getByRole("button", { name: "仍然上传" }));
  await waitFor(() => expect(submit).toHaveBeenCalledWith(expect.objectContaining({ dedup: expect.objectContaining({ channelId: "channel-a", bookId: "book-ai", allowDuplicate: true }) })));
});

it("blocks submission on lookup failure and allows a fresh retry", async () => {
  checkUpload.mockRejectedValueOnce({ message: "无法读取频道影片，请重试" });
  const submit = vi.fn();
  const view = render(<YouTubeUploadDialog batch={batch} sourcePath="/Downloads/merged.mp4" channelId="channel-a" onClose={vi.fn()} onSubmit={submit} />);
  fireEvent.click(view.getByRole("button", { name: "确认上传" }));
  await waitFor(() => expect(view.getByRole("alert").textContent).toContain("无法读取频道影片"));
  expect(submit).not.toHaveBeenCalled();
  expect(view.queryByRole("button", { name: "仍然上传" })).toBeNull();
  fireEvent.click(view.getByRole("button", { name: "确认上传" }));
  await waitFor(() => expect(submit).toHaveBeenCalledTimes(1));
});


it("discards a late lookup when the selected channel changes", async () => {
  let finish!: (matches: unknown[]) => void;
  checkUpload.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  const submit = vi.fn();
  const props = { batch, sourcePath: "/Downloads/merged.mp4", onClose: vi.fn(), onSubmit: submit };
  const view = render(<YouTubeUploadDialog {...props} channelId="channel-a" />);
  fireEvent.click(view.getByRole("button", { name: "确认上传" }));
  view.rerender(<YouTubeUploadDialog {...props} channelId="channel-b" />);
  await act(async () => { finish([]); });
  expect(submit).not.toHaveBeenCalled();
  fireEvent.click(view.getByRole("button", { name: "确认上传" }));
  await waitFor(() => expect(submit).toHaveBeenCalledWith(expect.objectContaining({ dedup: expect.objectContaining({ channelId: "channel-b" }) })));
});

it("skips a similar drama without enqueueing and invalidates matches after editing the title", async () => {
  checkUpload.mockResolvedValue([{ title: "AI 漫剧 全集", videoId: "similar", youtubeUrl: "https://www.youtube.com/watch?v=similar", reason: "similarTitle" }]);
  const submit = vi.fn();
  const close = vi.fn();
  const view = render(<YouTubeUploadDialog batch={batch} sourcePath="/Downloads/merged.mp4" channelId="channel-a" onClose={close} onSubmit={submit} />);
  fireEvent.click(view.getByRole("button", { name: "确认上传" }));
  await waitFor(() => expect(view.getByRole("alert").textContent).toContain("可能重复"));
  fireEvent.change(view.getByLabelText("YouTube 标题"), { target: { value: "新的标题" } });
  expect(view.queryByRole("button", { name: "仍然上传" })).toBeNull();
  fireEvent.click(view.getByRole("button", { name: "确认上传" }));
  await waitFor(() => expect(view.getByRole("button", { name: "跳过本次上传" })).toBeTruthy());
  fireEvent.click(view.getByRole("button", { name: "跳过本次上传" }));
  expect(close).toHaveBeenCalledTimes(1);
  expect(submit).not.toHaveBeenCalled();
});

it("ignores a completed lookup after the dialog is closed", async () => {
  let finish!: (matches: unknown[]) => void;
  checkUpload.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  const submit = vi.fn();
  const view = render(<YouTubeUploadDialog batch={batch} sourcePath="/Downloads/merged.mp4" channelId="channel-a" onClose={vi.fn()} onSubmit={submit} />);
  fireEvent.click(view.getByRole("button", { name: "确认上传" }));
  fireEvent.click(view.getByRole("button", { name: "取消" }));
  await act(async () => { finish([]); });
  expect(submit).not.toHaveBeenCalled();
});
