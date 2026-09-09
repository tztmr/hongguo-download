import { act, fireEvent, render, waitFor, within } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { SubtitlePicker, subtitleRequest } from "./SubtitlePicker";
import { YouTubeUploadDialog } from "./YouTubeUploadDialog";
import { YouTubeBatchUploadDialog } from "./YouTubeBatchUploadDialog";
import { YouTubeUploadJobs } from "./YouTubeUploadJobs";
import { createPreviewDownloadState, previewYouTubeModel } from "../preview";
import { readUploadPreferences } from "./uploadPreferences";
import { open } from "@tauri-apps/plugin-dialog";

const findSubtitle = vi.hoisted(() => vi.fn());
vi.mock("./subtitleCommands", () => ({ findYouTubeSubtitle: findSubtitle }));
vi.mock("./commands", () => ({ checkYouTubeUpload: vi.fn().mockResolvedValue([]) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
beforeEach(() => { const values = new Map<string, string>(); Object.defineProperty(window, "localStorage", { configurable: true, value: { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => values.set(key, value), clear: () => values.clear() } }); findSubtitle.mockReset().mockResolvedValue(null); vi.mocked(open).mockReset(); });
const batch = createPreviewDownloadState().batches[0];

it("remembers only general settings across single and batch dialogs", async () => {
  const props = { batch, sourcePath: "/video.mp4", channelId: "channel", onClose: vi.fn(), onSubmit: vi.fn().mockResolvedValue(undefined) };
  const view = render(<YouTubeUploadDialog {...props} />);
  fireEvent.change(view.getByLabelText("YouTube 可见性"), { target: { value: "unlisted" } });
  fireEvent.change(view.getByLabelText("YouTube 类别"), { target: { value: "24" } });
  fireEvent.change(view.getByLabelText("儿童受众"), { target: { value: "yes" } });
  fireEvent.change(view.getByLabelText("合成内容"), { target: { value: "no" } });
  fireEvent.change(view.getByLabelText("付费宣传内容"), { target: { value: "yes" } });
  fireEvent.change(view.getByLabelText("字幕语言"), { target: { value: "en" } });
  fireEvent.change(view.getByLabelText("YouTube 标题"), { target: { value: "不能缓存的标题" } });
  fireEvent.change(view.getByLabelText("YouTube 简介"), { target: { value: "不能缓存的简介" } });
  fireEvent.change(view.getByLabelText("YouTube 标签"), { target: { value: "不能缓存的标签" } });
  view.unmount();
  const second = render(<YouTubeBatchUploadDialog sources={[{ batch, sourcePath: "/another.mp4" }]} channelId="channel" onClose={vi.fn()} onSubmit={props.onSubmit} />);
  expect(second.getByLabelText("YouTube 可见性")).toHaveProperty("value", "unlisted");
  expect(second.getByLabelText("YouTube 类别")).toHaveProperty("value", "24");
  expect(second.getByLabelText("儿童受众")).toHaveProperty("value", "yes");
  expect(second.getByLabelText("合成内容")).toHaveProperty("value", "no");
  expect(second.getByLabelText("付费宣传内容")).toHaveProperty("value", "yes");
  expect(second.getByLabelText("字幕语言")).toHaveProperty("value", "en");
  expect(second.getByLabelText("YouTube 标题")).toHaveProperty("value", batch.title.slice(0, 100));
  expect(second.getByLabelText("YouTube 简介")).not.toHaveProperty("value", "不能缓存的简介");
  expect(second.getByLabelText("YouTube 标签")).not.toHaveProperty("value", "不能缓存的标签");
  expect(window.localStorage.getItem("hongguo.youtube.upload-preferences.v1")).not.toContain("不能缓存");
  fireEvent.click(second.getByRole("button", { name: "开始批量上传" }));
  await waitFor(() => expect(props.onSubmit).toHaveBeenCalledWith(expect.objectContaining({ privacyStatus: "unlisted", subtitle: { path: null, language: "en" } })));
});

it("no matching subtitle keeps video upload available; explicitly disabling omits subtitles", async () => {
  const submit = vi.fn();
  const view = render(<YouTubeUploadDialog batch={batch} sourcePath="/video.mp4" channelId="channel" onClose={vi.fn()} onSubmit={submit} />);
  await waitFor(() => expect(view.getByText("没有匹配字幕，将只上传视频")).toBeTruthy());
  expect(view.getByRole("button", { name: "确认上传" })).toHaveProperty("disabled", false);
  fireEvent.click(view.getByRole("button", { name: "不上传字幕" }));
  fireEvent.click(view.getByRole("button", { name: "确认上传" }));
  await waitFor(() => expect(submit).toHaveBeenCalledWith(expect.objectContaining({ subtitle: null, filePath: "/video.mp4" })));
});

it("selects an SRT and clears its association when video source changes", async () => {
  vi.mocked(open).mockResolvedValueOnce("/字幕.srt");
  const onChange = vi.fn();
  const props = { sourcePath: "/old.mp4", language: "zh-Hans", onLanguageChange: vi.fn(), onChange };
  const view = render(<SubtitlePicker {...props} />);
  fireEvent.click(view.getByRole("button", { name: "选择 SRT 字幕" }));
  await waitFor(() => expect(onChange).toHaveBeenCalledWith({ sourcePath: "/old.mp4", path: "/字幕.srt" }));
  const choice = onChange.mock.calls[0][0];
  view.rerender(<SubtitlePicker {...props} sourcePath="/new.mp4" value={choice} />);
  expect(view.queryByText("字幕文件：/字幕.srt")).toBeNull();
  expect(subtitleRequest(choice, "/new.mp4", "en")).toEqual({ path: null, language: "en" });
});

it("ignores a stale automatic lookup after a video switch", async () => {
  let finish!: (path: string) => void;
  findSubtitle.mockReturnValueOnce(new Promise<string>((resolve) => { finish = resolve; })).mockResolvedValueOnce("/new.srt");
  const props = { language: "zh-Hans", onLanguageChange: vi.fn(), onChange: vi.fn() };
  const view = render(<SubtitlePicker {...props} sourcePath="/old.mp4" />);
  view.rerender(<SubtitlePicker {...props} sourcePath="/new.mp4" />);
  await waitFor(() => expect(view.getByText("已匹配整季字幕：/new.srt")).toBeTruthy());
  await act(async () => { finish("/old.srt"); });
  expect(view.queryByText(/old.srt/)).toBeNull();
});

it("retries only subtitles on the existing video job", async () => {
  const uploadSubtitle = vi.fn().mockResolvedValue(undefined);
  const startUpload = vi.fn(); const retry = vi.fn();
  const model = { ...previewYouTubeModel, uploadSubtitle, startUpload, retry,
    jobs: [{ ...previewYouTubeModel.jobs[1], status: "videoUploadedSubtitleFailed" as const, subtitleState: "failed" as const, subtitleError: "请重新授权" }] };
  const view = render(<YouTubeUploadJobs model={model} onRevealPath={vi.fn()} />);
  fireEvent.click(view.getByRole("button", { name: "仅重试字幕" }));
  await waitFor(() => expect(uploadSubtitle).toHaveBeenCalledExactlyOnceWith(model.jobs[0].id, null));
  expect(startUpload).not.toHaveBeenCalled(); expect(retry).not.toHaveBeenCalled();
  fireEvent.click(view.getByRole("button", { name: "上传字幕" }));
  expect(within(view.getByRole("dialog", { name: "上传字幕" })).getByRole("button", { name: "选择 SRT 字幕" })).toBeTruthy();
});

it("falls back safely for corrupt or invalid cached settings", () => {
  window.localStorage.setItem("hongguo.youtube.upload-preferences.v1", "broken");
  expect(readUploadPreferences().privacy).toBe("private");
  window.localStorage.setItem("hongguo.youtube.upload-preferences.v1", JSON.stringify({ privacy: "invalid", madeForKids: "false", subtitleLanguage: "invalid language", title: "do not load" }));
  expect(readUploadPreferences()).toEqual({ privacy: "private", categoryId: "1", madeForKids: false, synthetic: true, paidPromotion: false, subtitleLanguage: "zh-Hans" });
});
