import { StrictMode } from "react";
import { act, fireEvent, render, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { YouTubeBatchUploadDialog, type YouTubeBatchUploadSource } from "./YouTubeBatchUploadDialog";
import type { YouTubeDuplicateMatch, YouTubeUploadIntent } from "./types";

// The only lookup boundary invokes Tauri / YouTube; submissions are supplied by the parent.
const checkUpload = vi.hoisted(() => vi.fn());
vi.mock("./commands", () => ({ checkYouTubeUpload: checkUpload }));
beforeEach(() => { const values = new Map<string, string>(); Object.defineProperty(window, "localStorage", { configurable: true, value: { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => values.set(key, value), clear: () => values.clear() } }); checkUpload.mockReset().mockResolvedValue([]); });

const sources: YouTubeBatchUploadSource[] = [
  {
    sourcePath: "/Downloads/都市全集.mp4",
    batch: {
      id: "batch-1", bookId: "book-1", title: "都市全集", cover: "", paused: false, createdAt: 1, updatedAt: 2,
      series: { bookId: "book-1", seriesId: "series-1", title: "都市归来", cover: "", abstract: "第一部简介", category: "真人剧", categoryTags: ["都市", "逆袭", "都市"], contentTypeCode: 1 },
      items: [],
    },
  },
  {
    sourcePath: "/Downloads/仙侠全集.mp4",
    batch: {
      id: "batch-2", bookId: "book-2", title: "仙侠全集", cover: "", paused: false, createdAt: 1, updatedAt: 2,
      series: { bookId: "book-2", seriesId: "series-2", title: "仙侠奇缘", cover: "", abstract: "", category: "仙侠·冒险，仙侠", contentTypeCode: 1 },
      items: [],
    },
  },
];
const duplicate: YouTubeDuplicateMatch[] = [{ title: "都市归来 全集", videoId: "existing", youtubeUrl: "https://www.youtube.com/watch?v=existing", reason: "sameDrama" }];

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function setup(selected = sources) {
  const props = {
    sources: selected, channelId: "channel-a",
    onSubmit: vi.fn<(request: YouTubeUploadIntent) => Promise<unknown>>().mockResolvedValue({}),
    onClose: vi.fn(), onQueued: vi.fn(),
  };
  const view = render(<StrictMode><YouTubeBatchUploadDialog {...props} /></StrictMode>);
  const row = (title: string) => within(view.getByRole("group", { name: title }));
  const start = () => fireEvent.click(view.getByRole("button", { name: "开始批量上传" }));
  return { ...view, props, row, start };
}

describe("YouTubeBatchUploadDialog", () => {
  it("defaults every row to the separated video and preserves an explicit override", async () => {
    const view = setup(sources.map((source) => ({ ...source, sourceOptions: [
      { kind: "merged" as const, path: source.sourcePath },
      { kind: "noBackgroundMusic" as const, path: source.sourcePath.replace(".mp4", "-clean.mp4") },
    ] })));
    expect(view.getByRole("button", { name: "开始批量上传" })).toHaveProperty("disabled", false);
    expect(view.row("都市归来").getByRole("radio", { name: "去背景音乐视频" })).toHaveProperty("checked", true);
    fireEvent.click(view.row("仙侠奇缘").getByRole("radio", { name: "合并视频（保留背景音乐）" }));
    view.start();
    await waitFor(() => expect(view.props.onSubmit).toHaveBeenCalledTimes(2));
    expect(view.props.onSubmit.mock.calls.map(([request]) => request.filePath)).toEqual(["/Downloads/都市全集-clean.mp4", "/Downloads/仙侠全集.mp4"]);
  });

  it("reviews every source and queues unique jobs sequentially with single-dialog defaults", async () => {
    const events: string[] = [];
    checkUpload.mockImplementation(async (query) => { events.push(`check:${query.bookId}`); return []; });
    const view = setup();
    view.props.onSubmit.mockImplementation(async (request) => { events.push(`queue:${request.dedup?.bookId}`); });
    expect(view.row("都市归来").getByText("上传文件：/Downloads/都市全集.mp4")).toBeTruthy();
    expect(view.row("仙侠奇缘").getByText("上传文件：/Downloads/仙侠全集.mp4")).toBeTruthy();
    expect(view.getByLabelText("付费宣传内容")).toHaveProperty("value", "no");
    view.start();
    await waitFor(() => expect(view.props.onQueued).toHaveBeenCalledWith(2));
    expect(events).toEqual(["check:book-1", "queue:book-1", "check:book-2", "queue:book-2"]);
    const requests = view.props.onSubmit.mock.calls.map(([request]) => request);
    expect(new Set(requests.map((request) => request.jobId)).size).toBe(2);
    expect(requests.every((request) => request.jobId.length > 0)).toBe(true);
    expect(requests[0]).toEqual({
      jobId: expect.any(String), filePath: "/Downloads/都市全集.mp4", coverPath: null,
      subtitle: { path: null, language: "zh-Hans" },
      title: "都市全集", description: "第一部简介", tags: ["都市", "逆袭", "都市归来"],
      categoryId: "1", privacyStatus: "private", selfDeclaredMadeForKids: false, containsSyntheticMedia: true, hasPaidProductPlacement: false,
      audienceConfirmed: true, syntheticMediaConfirmed: true, publishConfirmed: true,
      dedup: { channelId: "channel-a", bookId: "book-1", dramaTitle: "都市归来", allowDuplicate: false },
    });
    expect(requests[1]).toMatchObject({ title: "仙侠全集", description: "仙侠全集", tags: ["仙侠", "冒险", "仙侠奇缘"] });
    expect(checkUpload).toHaveBeenNthCalledWith(1, { channelId: "channel-a", bookId: "book-1", dramaTitle: "都市归来", title: "都市全集" });
    expect(checkUpload).toHaveBeenNthCalledWith(2, { channelId: "channel-a", bookId: "book-2", dramaTitle: "仙侠奇缘", title: "仙侠全集" });
    expect(view.getAllByText("已加入上传队列")).toHaveLength(2);
    expect(view.props.onClose).not.toHaveBeenCalled();
    view.start();
    expect(view.props.onSubmit).toHaveBeenCalledTimes(2);
  });

  it("submits per-item edits and shared metadata without changing drama identity", async () => {
    const view = setup();
    fireEvent.change(view.row("都市归来").getByLabelText("YouTube 标题"), { target: { value: " 新标题 " } });
    fireEvent.change(view.row("都市归来").getByLabelText("YouTube 简介"), { target: { value: "新简介" } });
    fireEvent.change(view.row("都市归来").getByLabelText("YouTube 标签"), { target: { value: " 原创，测试, , " } });
    fireEvent.change(view.getByLabelText("YouTube 可见性"), { target: { value: "unlisted" } });
    fireEvent.change(view.getByLabelText("YouTube 类别"), { target: { value: "24" } });
    fireEvent.change(view.getByLabelText("儿童受众"), { target: { value: "yes" } });
    fireEvent.change(view.getByLabelText("合成内容"), { target: { value: "no" } });
    fireEvent.change(view.getByLabelText("付费宣传内容"), { target: { value: "yes" } });
    view.start();
    await waitFor(() => expect(view.props.onQueued).toHaveBeenCalledWith(2));
    expect(view.props.onSubmit.mock.calls[0][0]).toMatchObject({ title: "新标题", description: "新简介", tags: ["原创", "测试"], dedup: { dramaTitle: "都市归来" } });
    expect(view.props.onSubmit.mock.calls[1][0]).toMatchObject({ title: "仙侠全集", description: "仙侠全集" });
    for (const [request] of view.props.onSubmit.mock.calls) {
      expect(request).toMatchObject({ privacyStatus: "unlisted", categoryId: "24", selfDeclaredMadeForKids: true, containsSyntheticMedia: false, hasPaidProductPlacement: true });
    }
    expect(checkUpload.mock.calls[0][0].title).toBe("新标题");
  });

  it("skips duplicates, then rechecks only the explicitly overridden item", async () => {
    checkUpload.mockResolvedValueOnce(duplicate);
    const view = setup();
    view.start();
    await waitFor(() => expect(view.props.onQueued).toHaveBeenCalledWith(1));
    expect(view.props.onSubmit).toHaveBeenCalledTimes(1);
    expect(view.props.onSubmit.mock.calls[0][0].dedup?.bookId).toBe("book-2");
    expect(view.row("都市归来").getByRole("alert").textContent).toContain("重复");
    expect(view.row("都市归来").getByText("都市归来 全集")).toBeTruthy();
    expect(view.props.onClose).not.toHaveBeenCalled();
    view.start();
    expect(checkUpload).toHaveBeenCalledTimes(2);
    checkUpload.mockResolvedValueOnce(duplicate);
    fireEvent.click(view.row("都市归来").getByRole("button", { name: "仍然上传" }));
    await waitFor(() => expect(view.props.onSubmit).toHaveBeenCalledTimes(2));
    expect(checkUpload).toHaveBeenCalledTimes(3);
    expect(view.props.onSubmit.mock.calls[1][0].dedup).toEqual({ channelId: "channel-a", bookId: "book-1", dramaTitle: "都市归来", allowDuplicate: true });
    expect(view.props.onQueued.mock.calls).toEqual([[1], [1]]);
  });

  it("continues after check and enqueue failures; retry rechecks failures and never resubmits queued jobs", async () => {
    const third = { ...sources[1], sourcePath: "/Downloads/third.mp4", batch: { ...sources[1].batch, id: "batch-3", bookId: "book-3", series: { ...sources[1].batch.series, title: "第三部" } } };
    checkUpload.mockRejectedValueOnce({ message: "频道检查失败" });
    const view = setup([...sources, third]);
    view.props.onSubmit.mockRejectedValueOnce(new Error("入队失败"));
    view.start();
    await waitFor(() => expect(view.props.onQueued).toHaveBeenCalledWith(1));
    expect(view.row("都市归来").getByRole("alert").textContent).toContain("频道检查失败");
    expect(view.row("仙侠奇缘").getByRole("alert").textContent).toContain("入队失败");
    expect(view.row("第三部").getByText("已加入上传队列")).toBeTruthy();
    expect(view.queryByRole("button", { name: "仍然上传" })).toBeNull();
    const failedJobId = view.props.onSubmit.mock.calls[0][0].jobId;
    view.start();
    await waitFor(() => expect(view.props.onQueued).toHaveBeenLastCalledWith(2));
    expect(checkUpload.mock.calls.map(([query]) => query.bookId)).toEqual(["book-1", "book-2", "book-3", "book-1", "book-2"]);
    expect(view.props.onSubmit.mock.calls.map(([request]) => request.dedup?.bookId)).toEqual(["book-2", "book-3", "book-1", "book-2"]);
    expect(view.props.onSubmit.mock.calls[3][0].jobId).toBe(failedJobId);
    expect(view.props.onSubmit.mock.calls.every(([request]) => request.dedup?.allowDuplicate === false)).toBe(true);
    expect(view.props.onClose).not.toHaveBeenCalled();
  });

  it("does not carry duplicate consent across a failed check and retry", async () => {
    checkUpload.mockResolvedValueOnce(duplicate);
    const view = setup([sources[0]]);
    view.start();
    await waitFor(() => expect(view.row("都市归来").getByRole("button", { name: "仍然上传" })).toBeTruthy());
    checkUpload.mockRejectedValueOnce(new Error("无法查重"));
    fireEvent.click(view.row("都市归来").getByRole("button", { name: "仍然上传" }));
    await waitFor(() => expect(view.getByRole("alert").textContent).toContain("无法查重"));
    expect(view.props.onSubmit).not.toHaveBeenCalled();
    expect(view.queryByRole("button", { name: "仍然上传" })).toBeNull();
    checkUpload.mockResolvedValueOnce(duplicate);
    view.start();
    await waitFor(() => expect(view.getByRole("button", { name: "仍然上传" })).toBeTruthy());
    expect(checkUpload).toHaveBeenCalledTimes(3);
    expect(view.props.onSubmit).not.toHaveBeenCalled();
    expect(view.props.onQueued).not.toHaveBeenCalled();
  });

  it("invalidates duplicate consent when the title is edited", async () => {
    checkUpload.mockResolvedValueOnce(duplicate);
    const view = setup([sources[0]]);
    view.start();
    await waitFor(() => expect(view.getByRole("button", { name: "仍然上传" })).toBeTruthy());
    fireEvent.change(view.getByLabelText("YouTube 标题"), { target: { value: "另一部标题" } });
    expect(view.queryByRole("button", { name: "仍然上传" })).toBeNull();
    view.start();
    await waitFor(() => expect(view.props.onQueued).toHaveBeenCalledWith(1));
    expect(checkUpload).toHaveBeenLastCalledWith(expect.objectContaining({ title: "另一部标题" }));
    expect(view.props.onSubmit.mock.calls[0][0].dedup?.allowDuplicate).toBe(false);
  });

  it("guards rapid starts throughout checking and submitting and keeps equivalent parent rerenders stable", async () => {
    const lookup = deferred<YouTubeDuplicateMatch[]>();
    const submission = deferred<unknown>();
    checkUpload.mockReturnValueOnce(lookup.promise);
    const view = setup();
    view.props.onSubmit.mockReturnValueOnce(submission.promise);
    view.start(); view.start();
    view.rerender(<StrictMode><YouTubeBatchUploadDialog {...view.props} sources={[...sources]} /></StrictMode>);
    expect(checkUpload).toHaveBeenCalledTimes(1);
    expect((view.row("都市归来").getByLabelText("YouTube 标题") as HTMLInputElement).closest("fieldset")?.disabled).toBe(true);
    await act(async () => { lookup.resolve([]); });
    view.start();
    expect(view.props.onSubmit).toHaveBeenCalledTimes(1);
    expect(checkUpload).toHaveBeenCalledTimes(1);
    await act(async () => { submission.resolve({}); });
    await waitFor(() => expect(view.props.onQueued).toHaveBeenCalledWith(2));
    expect(view.props.onSubmit).toHaveBeenCalledTimes(2);
  });

  it.each(["cancel", "unmount", "channel", "source"])("discards late checks on %s", async (action) => {
    const lookup = deferred<YouTubeDuplicateMatch[]>();
    checkUpload.mockReturnValueOnce(lookup.promise);
    const view = setup();
    view.start();
    if (action === "cancel") fireEvent.click(view.getByRole("button", { name: "取消" }));
    if (action === "unmount") view.unmount();
    if (action === "channel") view.rerender(<StrictMode><YouTubeBatchUploadDialog {...view.props} channelId="channel-b" /></StrictMode>);
    if (action === "source") view.rerender(<StrictMode><YouTubeBatchUploadDialog {...view.props} sources={[sources[1]]} /></StrictMode>);
    await act(async () => { lookup.resolve([]); });
    expect(view.props.onSubmit).not.toHaveBeenCalled();
    expect(checkUpload).toHaveBeenCalledTimes(1);
    expect(view.props.onQueued).not.toHaveBeenCalled();
    if (action === "cancel") expect(view.props.onClose).toHaveBeenCalledTimes(1);
  });

  it("stops remaining work when dismissed during an enqueue", async () => {
    const submission = deferred<unknown>();
    const view = setup();
    view.props.onSubmit.mockReturnValueOnce(submission.promise);
    view.start();
    await waitFor(() => expect(view.props.onSubmit).toHaveBeenCalledTimes(1));
    fireEvent.click(view.getByRole("button", { name: "关闭" }));
    await act(async () => { submission.resolve({}); });
    expect(view.props.onClose).toHaveBeenCalledTimes(1);
    expect(checkUpload).toHaveBeenCalledTimes(1);
    expect(view.props.onSubmit).toHaveBeenCalledTimes(1);
  });

  it("holds unconfirmed publishing and empty selections without any boundary calls", () => {
    const view = setup();
    fireEvent.click(view.getByLabelText("我确认将这些视频发布到所选 YouTube 频道"));
    view.start();
    expect(checkUpload).not.toHaveBeenCalled();
    view.unmount();
    const empty = setup([]);
    empty.start();
    expect(checkUpload).not.toHaveBeenCalled();
    expect(empty.props.onSubmit).not.toHaveBeenCalled();
  });

  it("reports an empty title per item, allows repair, and keeps queued items locked", async () => {
    const view = setup();
    fireEvent.change(view.row("都市归来").getByLabelText("YouTube 标题"), { target: { value: "   " } });
    view.start();
    await waitFor(() => expect(view.props.onQueued).toHaveBeenCalledWith(1));
    expect(view.row("都市归来").getByRole("alert").textContent).toContain("标题");
    expect(checkUpload).toHaveBeenCalledTimes(1);
    expect((view.row("仙侠奇缘").getByLabelText("YouTube 标题") as HTMLInputElement).closest("fieldset")?.disabled).toBe(true);
    fireEvent.change(view.row("都市归来").getByLabelText("YouTube 标题"), { target: { value: "修正标题" } });
    view.start();
    await waitFor(() => expect(view.props.onSubmit).toHaveBeenCalledTimes(2));
    expect(view.props.onSubmit.mock.calls[1][0].title).toBe("修正标题");
  });
});
