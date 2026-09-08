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

function stripUnixPrivatePrefix(path: string): string {
  for (const prefix of ["/private/tmp", "/private/var", "/private/etc"]) {
    if (path === prefix || path.startsWith(`${prefix}/`)) {
      return path.slice("/private".length);
    }
  }
  return path;
}

export function normalizeFsPath(path: string): string {
  if (!path) return "";
  let normalized = path.replace(/\\/g, "/");
  const upper = normalized.toUpperCase();
  if (upper.startsWith("//?/UNC/")) {
    normalized = `//${normalized.slice("//?/UNC/".length)}`;
  } else if (upper.startsWith("//?/")) {
    normalized = normalized.slice("//?/".length);
  }
  if (/^[a-zA-Z]:/.test(normalized)) {
    normalized = `${normalized[0].toUpperCase()}${normalized.slice(1)}`;
  }
  return stripUnixPrivatePrefix(normalized);
}

function isWindowsLikePath(path: string): boolean {
  return /^[A-Z]:/i.test(path) || path.startsWith("//");
}

export function sameFsPath(left?: string | null, right?: string | null): boolean {
  if (!left || !right) return false;
  const first = normalizeFsPath(left);
  const second = normalizeFsPath(right);
  if (first === second) return true;
  return (isWindowsLikePath(first) || isWindowsLikePath(second)) && first.toLowerCase() === second.toLowerCase();
}

export function pathIsWithin(root?: string | null, candidate?: string | null): boolean {
  if (!root || !candidate) return false;
  const parent = normalizeFsPath(root).replace(/\/+$/, "");
  const child = normalizeFsPath(candidate);
  if (!parent || !child) return false;
  const [left, right] = isWindowsLikePath(parent) || isWindowsLikePath(child)
    ? [parent.toLowerCase(), child.toLowerCase()]
    : [parent, child];
  return right === left || right.startsWith(`${left}/`);
}
