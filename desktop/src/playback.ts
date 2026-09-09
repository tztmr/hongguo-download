import { invoke } from "@tauri-apps/api/core";
import type { DefinitionPreference } from "./types";

export async function loadEpisodeVideo(itemId: string, definition: DefinitionPreference, signal: AbortSignal): Promise<Blob> {
  const url = await invoke<string>("get_playback_url", { itemId, definition });
  signal.throwIfAborted();
  const response = await fetch(url, { signal, cache: "no-store" });
  if (!response.ok || !response.headers.get("content-type")?.startsWith("video/")) {
    const detail = await response.json().catch(() => null);
    throw new Error(detail?.msg || `视频加载失败（HTTP ${response.status}）`);
  }
  const blob = await response.blob();
  if (!blob.size) throw new Error("视频内容为空，请重试");
  return blob;
}
