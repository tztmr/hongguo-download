import { invoke } from "@tauri-apps/api/core";
import type { DefinitionPreference } from "./types";

export type StreamingVideo = { mediaSource: MediaSource; finished: Promise<void>; currentTime: () => number };

function mediaEvent(target: EventTarget, event: string, signal: AbortSignal, action?: () => void): Promise<void> {
  return new Promise((resolve, reject) => {
    const cleanup = () => {
      target.removeEventListener(event, done);
      target.removeEventListener("error", failed);
      signal.removeEventListener("abort", aborted);
    };
    const done = () => { cleanup(); resolve(); };
    const failed = () => { cleanup(); reject(new Error("视频画面解码失败，请重试或切换剧集")); };
    const aborted = () => { cleanup(); reject(signal.reason); };
    target.addEventListener(event, done, { once: true });
    target.addEventListener("error", failed, { once: true });
    signal.addEventListener("abort", aborted, { once: true });
    if (signal.aborted) aborted();
    else if (action) {
      try { action(); } catch (reason) { cleanup(); reject(reason); }
    }
  });
}

function progressiveVideo(response: Response, signal: AbortSignal): StreamingVideo {
  const mime = response.headers.get("x-playback-mime");
  if (!mime || !MediaSource.isTypeSupported(mime) || !response.body) {
    void response.body?.cancel();
    throw new Error("当前系统不支持此视频流，请更新 Microsoft Edge WebView2 后重试");
  }
  const mediaSource = new MediaSource();
  const opened = mediaEvent(mediaSource, "sourceopen", signal);
  const reader = response.body.getReader();
  const cancel = () => { void reader.cancel().catch(() => {}); };
  signal.addEventListener("abort", cancel, { once: true });
  const session: StreamingVideo = { mediaSource, finished: Promise.resolve(), currentTime: () => 0 };
  const finished = (async () => {
    try {
      await opened;
      signal.throwIfAborted();
      const buffer = mediaSource.addSourceBuffer(mime);
      const duration = Number(response.headers.get("x-playback-duration"));
      if (Number.isFinite(duration) && duration > 0) mediaSource.duration = duration;
      let received = 0;
      while (true) {
        const { done, value } = await reader.read();
        signal.throwIfAborted();
        if (done) break;
        received += value.byteLength;
        while (true) {
          try {
            await mediaEvent(buffer, "updateend", signal, () => buffer.appendBuffer(value));
            break;
          } catch (reason) {
            if (!(reason instanceof DOMException) || reason.name !== "QuotaExceededError") throw reason;
            // Keep a rewind window; never evict frames the viewer hasn't watched.
            const end = session.currentTime() - 30;
            if (buffer.buffered.length && end > buffer.buffered.start(0)) {
              await mediaEvent(buffer, "updateend", signal, () => buffer.remove(0, end));
            } else {
              await new Promise<void>((resolve) => setTimeout(resolve, 250));
              signal.throwIfAborted();
            }
          }
        }
      }
      if (!received) throw new Error("视频内容为空，请重试");
      if (mediaSource.readyState === "open") mediaSource.endOfStream();
    } finally {
      signal.removeEventListener("abort", cancel);
      await reader.cancel().catch(() => {});
      reader.releaseLock();
    }
  })();
  // The caller attaches after creating the media URL; cancellation can win that race.
  void finished.catch(() => {});
  session.finished = finished;
  return session;
}

export async function loadEpisodeVideo(itemId: string, definition: DefinitionPreference, signal: AbortSignal): Promise<Blob | StreamingVideo> {
  const url = await invoke<string>("get_playback_url", { itemId, definition });
  signal.throwIfAborted();
  const playbackUrl = new URL(url);
  const stream = playbackUrl.searchParams.get("playback_compat") === "true" && typeof MediaSource !== "undefined";
  if (stream) playbackUrl.searchParams.set("playback_stream", "true");
  const response = await fetch(stream ? playbackUrl.toString() : url, { signal, cache: "no-store" });
  if (!response.ok || !response.headers.get("content-type")?.startsWith("video/")) {
    const detail = await response.json().catch(() => null);
    throw new Error(detail?.msg || `视频加载失败（HTTP ${response.status}）`);
  }
  signal.throwIfAborted();
  if (stream) return progressiveVideo(response, signal);
  const blob = await response.blob();
  if (!blob.size) throw new Error("视频内容为空，请重试");
  return blob;
}
