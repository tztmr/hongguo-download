import type { DownloadBatch } from "../download/model";
import type { StartMergeInput } from "./types";

export function completedMergeInputs(batch: DownloadBatch): StartMergeInput[] {
  return batch.items
    .filter((item) => item.status === "done" && Boolean(item.path))
    .slice()
    .sort((left, right) => left.episodeIndex - right.episodeIndex)
    .map((item) => ({ episodeIndex: item.episodeIndex, path: item.path as string }));
}

export function seriesRootFromInputs(inputs: StartMergeInput[]): string {
  const first = inputs[0]?.path || "";
  let separator = -1;
  for (let index = first.length - 1; index >= 0; index -= 1) {
    const code = first.charCodeAt(index);
    if (code === 47 || code === 92) {
      separator = index;
      break;
    }
  }
  return separator >= 0 ? first.slice(0, separator) : first;
}

export function safeOutputFileName(name: string): string {
  const cleaned = Array.from(name, (char) => {
    const code = char.charCodeAt(0);
    return code === 92 || code === 47 || code === 58 || code === 42 || code === 63 || code === 34 || code === 60 || code === 62 || code === 124
      ? " "
      : char;
  }).join("").replace(/\s+/g, " ").trim() || "未命名";
  return cleaned.toLowerCase().endsWith(".mp4") ? cleaned : `${cleaned}.mp4`;
}

export function isCompletedBatch(batch: DownloadBatch): boolean {
  return batch.items.length > 0 && batch.items.every((item) => item.status === "done" && Boolean(item.path));
}
