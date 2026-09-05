import { renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { NotificationAdapter, NotificationTarget } from "../notifications";
import type { MediaJobsModel } from "../media/types";
import type { YouTubeModel } from "../youtube/types";
import { useNotificationRouter } from "./useNotificationRouter";

function mediaFixture(): MediaJobsModel {
  return {
    jobs: [{
      id: "media-1", dedupeKey: "d", kind: "extractSubtitles", status: "completed", stage: "completed", percent: 100,
      inputs: [], outputPath: null, errorCode: null, errorMessage: null,
      outputs: Array.from({ length: 20 }, (_, index) => ({ episodeIndex: index + 1, kind: "subtitles" as const, path: `/safe/${index}.srt` })),
    }],
    startMerge: vi.fn(), startAudioSeparation: vi.fn(), startSubtitleExtraction: vi.fn(), cancel: vi.fn(), retry: vi.fn(),
    markNotified: vi.fn().mockResolvedValue(undefined),
  };
}

function youtubeFixture(): YouTubeModel {
  return {
    credential: { configured: true, clientIdSuffix: "…123456" }, channels: [], activeChannelId: "UC_TEST", loading: false, busy: false,
    jobs: [{ id: "youtube-1", title: "测试剧", channelId: "UC_TEST", sourcePath: "/safe/video.mp4", status: "videoUploadedThumbnailFailed", uploadedBytes: 10, totalBytes: 10, percent: 100, errorCode: "THUMBNAIL_FORBIDDEN", errorMessage: "safe", videoId: "abc", youtubeUrl: "https://youtu.be/abc", actualPrivacyStatus: "private", thumbnailState: "failed" }],
    importCredential: vi.fn(), authorize: vi.fn(), setChannel: vi.fn(), revoke: vi.fn(), removeCredential: vi.fn(), startUpload: vi.fn(), cancel: vi.fn(), retry: vi.fn(), retryThumbnail: vi.fn(),
    markNotified: vi.fn().mockResolvedValue(undefined),
  };
}

describe("useNotificationRouter", () => {
  it("aggregates terminal media and partial YouTube outcomes exactly once per render cycle", async () => {
    const send = vi.fn().mockResolvedValue(true);
    const adapter: NotificationAdapter = { getStatus: vi.fn(), send };
    const media = mediaFixture();
    const youtube = youtubeFixture();
    const view = renderHook(() => useNotificationRouter({ adapter, media, youtube, notifyMedia: true, notifyYouTube: true, onTarget: vi.fn() }));
    await waitFor(() => expect(send).toHaveBeenCalledTimes(2));
    expect(send).toHaveBeenCalledWith(expect.objectContaining({ title: "字幕提取完成", body: expect.stringContaining("20 个结果") }));
    expect(send).toHaveBeenCalledWith(expect.objectContaining({ title: "视频已上传，封面失败" }));
    expect(media.markNotified).toHaveBeenCalledTimes(1);
    expect(youtube.markNotified).toHaveBeenCalledTimes(1);
    view.rerender();
    expect(send).toHaveBeenCalledTimes(2);
  });

  it("routes notification actions through one subscribed listener and cleans it up", async () => {
    let listener: ((target: NotificationTarget) => void) | undefined;
    const stop = vi.fn();
    const adapter: NotificationAdapter = {
      getStatus: vi.fn(), send: vi.fn(),
      subscribeActions: vi.fn(async (next) => { listener = next; return stop; }),
    };
    const onTarget = vi.fn();
    const view = renderHook(() => useNotificationRouter({ adapter, media: { ...mediaFixture(), jobs: [] }, youtube: { ...youtubeFixture(), jobs: [] }, notifyMedia: false, notifyYouTube: false, onTarget }));
    await waitFor(() => expect(adapter.subscribeActions).toHaveBeenCalledTimes(1));
    listener?.({ kind: "mediaJob", id: "media-1" });
    expect(onTarget).toHaveBeenCalledWith({ kind: "mediaJob", id: "media-1" });
    view.unmount();
    expect(stop).toHaveBeenCalledTimes(1);
  });

  it("claims terminal outcomes without sending when result toggles are disabled", async () => {
    const adapter: NotificationAdapter = { getStatus: vi.fn(), send: vi.fn() };
    const media = mediaFixture();
    const youtube = youtubeFixture();
    renderHook(() => useNotificationRouter({ adapter, media, youtube, notifyMedia: false, notifyYouTube: false, onTarget: vi.fn() }));
    await waitFor(() => expect(media.markNotified).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(youtube.markNotified).toHaveBeenCalledTimes(1));
    expect(adapter.send).not.toHaveBeenCalled();
  });
});
