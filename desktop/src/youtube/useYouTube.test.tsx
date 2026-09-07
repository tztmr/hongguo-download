import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { YouTubeCommands, YouTubeJob, YouTubeSnapshot } from "./types";
import { useYouTube } from "./useYouTube";

const job: YouTubeJob = {
  id: "youtube-1", title: "测试剧", channelId: "UC_TEST", sourcePath: "/Downloads/merged.mp4",
  status: "queued", uploadedBytes: 0, totalBytes: 100, percent: 0, errorCode: null,
  errorMessage: null, videoId: null, youtubeUrl: null, actualPrivacyStatus: null, thumbnailState: "pending",
};
const snapshot: YouTubeSnapshot = {
  credential: { configured: true, clientIdSuffix: "…123456" },
  channels: [{ channelId: "UC_TEST", title: "测试频道", authorizedAt: "2026-09-04T00:00:00Z" }],
  activeChannelId: "UC_TEST",
  jobs: [job],
};

function fixture() {
  let listener: ((value: YouTubeJob) => void) | undefined;
  const commands: YouTubeCommands & { emit(value: YouTubeJob): void } = {
    snapshot: vi.fn().mockResolvedValue(snapshot),
    importCredential: vi.fn().mockResolvedValue(snapshot.credential), authorize: vi.fn().mockResolvedValue(snapshot.channels[0]),
    setChannel: vi.fn().mockResolvedValue(snapshot), revoke: vi.fn().mockResolvedValue(snapshot),
    removeCredential: vi.fn().mockResolvedValue(snapshot), startUpload: vi.fn().mockResolvedValue(job),
    pause: vi.fn().mockResolvedValue({ ...job, status: "paused" }), resume: vi.fn().mockResolvedValue(job),
    cancel: vi.fn().mockResolvedValue(undefined), retry: vi.fn().mockResolvedValue(job), retryThumbnail: vi.fn().mockResolvedValue(job),
    subscribeProgress: vi.fn(async (next) => { listener = next; return vi.fn(); }),
    emit: (value) => listener?.(value),
  };
  return commands;
}

describe("useYouTube", () => {
  it("clears a previous callback timeout while retrying and shows the connected channel", async () => {
    const commands = fixture();
    vi.mocked(commands.authorize).mockRejectedValueOnce({ code: "OAUTH_CALLBACK_TIMEOUT", message: "YouTube 授权回调超时" });
    const { result } = renderHook(() => useYouTube(commands));
    await waitFor(() => expect(result.current.loading).toBe(false));
    await act(async () => { await expect(result.current.authorize()).rejects.toMatchObject({ code: "OAUTH_CALLBACK_TIMEOUT" }); });
    expect(result.current.error?.code).toBe("OAUTH_CALLBACK_TIMEOUT");
    let complete!: (channel: typeof snapshot.channels[number]) => void;
    vi.mocked(commands.authorize).mockImplementationOnce(() => new Promise((resolve) => { complete = resolve; }));
    let pending!: Promise<void>;
    act(() => { pending = result.current.authorize(); });
    expect(result.current.busy).toBe(true);
    expect(result.current.error).toBeUndefined();
    await act(async () => {
      complete({ channelId: "UC_NEW", title: "新频道", authorizedAt: "2026-09-05T00:00:00Z" });
      await pending;
    });
    expect(result.current.busy).toBe(false);
    expect(result.current.error).toBeUndefined();
    expect(result.current.activeChannelId).toBe("UC_NEW");
  });

  it("loads the safe snapshot and replaces progress events by job id", async () => {
    const commands = fixture();
    const { result } = renderHook(() => useYouTube(commands));
    await waitFor(() => expect(result.current.jobs).toEqual([job]));
    act(() => commands.emit({ ...job, status: "uploading", percent: 40, uploadedBytes: 40 }));
    await waitFor(() => expect(result.current.jobs[0].percent).toBe(40));
  });

  it("routes upload, retry, cancel, and thumbnail retry by stable job id", async () => {
    const commands = fixture();
    const { result } = renderHook(() => useYouTube(commands));
    await waitFor(() => expect(result.current.loading).toBe(false));
    const request = {
      jobId: "youtube-1", filePath: "/Downloads/merged.mp4", coverPath: null, title: "测试剧", description: "",
      tags: ["短剧"], categoryId: "24", privacyStatus: "private" as const, selfDeclaredMadeForKids: false,
      containsSyntheticMedia: false, audienceConfirmed: true, syntheticMediaConfirmed: true, publishConfirmed: true,
    };
    await act(async () => {
      await result.current.startUpload(request);
      await result.current.pause("youtube-1");
      await result.current.resume("youtube-1");
      await result.current.cancel("youtube-1");
      await result.current.retry("youtube-1");
      await result.current.retryThumbnail("youtube-1");
    });
    expect(commands.startUpload).toHaveBeenCalledWith(request);
    expect(commands.pause).toHaveBeenCalledWith("youtube-1");
    expect(commands.resume).toHaveBeenCalledWith("youtube-1");
    expect(commands.cancel).toHaveBeenCalledWith("youtube-1");
    expect(commands.retry).toHaveBeenCalledWith("youtube-1");
    expect(commands.retryThumbnail).toHaveBeenCalledWith("youtube-1");
  });

  it("keeps the completed pause event when the IPC reply arrives later", async () => {
    const commands = fixture();
    const { result } = renderHook(() => useYouTube(commands));
    await waitFor(() => expect(result.current.loading).toBe(false));
    let resolve!: (job: YouTubeJob) => void;
    vi.mocked(commands.pause).mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    let pending!: Promise<void>;
    act(() => { pending = result.current.pause(job.id); });
    act(() => commands.emit({ ...job, status: "paused", uploadedBytes: 40, percent: 40 }));
    await act(async () => { resolve({ ...job, status: "pausing" }); await pending; });
    expect(result.current.jobs[0]).toMatchObject({ status: "paused", uploadedBytes: 40 });
  });

  it("does not expose arbitrary rejection text when no stable error code exists", async () => {
    const commands = fixture();
    vi.mocked(commands.authorize).mockRejectedValue(new Error("Bearer secret-value"));
    const { result } = renderHook(() => useYouTube(commands));
    await waitFor(() => expect(result.current.loading).toBe(false));
    await act(async () => { await expect(result.current.authorize()).rejects.toMatchObject({ code: "UNKNOWN_ERROR" }); });
    expect(result.current.error).toEqual({ code: "UNKNOWN_ERROR", message: "YouTube 操作失败，请重试" });
  });
});
