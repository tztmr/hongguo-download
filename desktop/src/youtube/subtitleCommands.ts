import { invoke, isTauri } from "@tauri-apps/api/core";
export const findYouTubeSubtitle = (sourcePath: string) => isTauri()
  ? invoke<string | null>("find_youtube_subtitle", { sourcePath }) : Promise.resolve(null);
