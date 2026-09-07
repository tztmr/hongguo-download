import {
  clampConcurrency,
  createInitialState,
  type DownloadBatch,
  type DownloadItem,
  type DownloadItemStatus,
  type DownloadManagerState,
  type DownloadSeriesSnapshot,
} from "./model";

export const DOWNLOAD_STORAGE_KEY = "hongguo.downloads.v1";
const CORRUPT_KEY_PREFIX = "hongguo.downloads.corrupt.";
const statuses = new Set<DownloadItemStatus>(["queued", "running", "done", "error"]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function readItem(value: unknown): DownloadItem {
  if (!isRecord(value) || typeof value.id !== "string" || typeof value.itemId !== "string") {
    throw new Error("invalid download item");
  }
  if (typeof value.status !== "string" || !statuses.has(value.status as DownloadItemStatus)) {
    throw new Error("invalid download status");
  }
  const restoredStatus = value.status === "running" ? "queued" : (value.status as DownloadItemStatus);
  const resetProgress = value.status === "running";
  return {
    id: value.id,
    itemId: value.itemId,
    episodeIndex: Number(value.episodeIndex || 0),
    episodeTitle: typeof value.episodeTitle === "string" ? value.episodeTitle : "未命名剧集",
    definition: typeof value.definition === "string" ? value.definition : "1080p",
    status: restoredStatus,
    percent: resetProgress ? 0 : Number(value.percent || 0),
    received: resetProgress ? 0 : Number(value.received || 0),
    total: resetProgress || typeof value.total !== "number" ? undefined : value.total,
    path: typeof value.path === "string" ? value.path : undefined,
    error: typeof value.error === "string" ? value.error : undefined,
    addedAt: Number(value.addedAt || Date.now()),
    completedAt: typeof value.completedAt === "number" ? value.completedAt : undefined,
  };
}

function readSeriesSnapshot(value: unknown): DownloadSeriesSnapshot {
  if (
    !isRecord(value) ||
    typeof value.bookId !== "string" ||
    typeof value.seriesId !== "string" ||
    typeof value.title !== "string" ||
    typeof value.cover !== "string" ||
    typeof value.abstract !== "string" ||
    typeof value.category !== "string" ||
    typeof value.contentTypeCode !== "number"
  ) {
    throw new Error("invalid download series snapshot");
  }
  return {
    bookId: value.bookId,
    seriesId: value.seriesId,
    title: value.title,
    cover: value.cover,
    abstract: value.abstract,
    category: value.category,
    categoryTags: Array.isArray(value.categoryTags)
      ? value.categoryTags.filter((tag): tag is string => typeof tag === "string")
      : undefined,
    contentTypeCode: value.contentTypeCode,
    episodeCount: typeof value.episodeCount === "number" ? value.episodeCount : 0,
    durationSeconds: typeof value.durationSeconds === "number" ? value.durationSeconds : undefined,
    author: typeof value.author === "string" ? value.author : "",
    onlineTime: typeof value.onlineTime === "number" ? value.onlineTime : undefined,
    releaseType: typeof value.releaseType === "string" ? value.releaseType as DownloadSeriesSnapshot["releaseType"] : undefined,
  };
}

function readBatch(value: unknown, version: 1 | 2): DownloadBatch {
  if (
    !isRecord(value) ||
    typeof value.id !== "string" ||
    typeof value.bookId !== "string" ||
    typeof value.title !== "string" ||
    !Array.isArray(value.items)
  ) {
    throw new Error("invalid download batch");
  }
  const items = value.items.map(readItem);
  const updatedAt = Number(value.updatedAt || Date.now());
  const cover = typeof value.cover === "string" ? value.cover : "";
  return {
    id: value.id,
    bookId: value.bookId,
    title: value.title,
    cover,
    series: version === 1
      ? {
          bookId: value.bookId,
          seriesId: value.bookId,
          title: value.title,
          cover,
          abstract: "",
          category: "",
          contentTypeCode: 1,
          episodeCount: items.length,
          author: "",
        }
      : readSeriesSnapshot(value.series),
    paused: Boolean(value.paused),
    items,
    createdAt: Number(value.createdAt || Date.now()),
    updatedAt,
    removeWhenIdle: value.removeWhenIdle === true ? true : undefined,
    completionNotifiedAt: typeof value.completionNotifiedAt === "number"
      ? value.completionNotifiedAt
      : items.length > 0 && items.every((item) => item.status === "done")
        ? updatedAt
        : undefined,
  };
}

function parseState(value: unknown): DownloadManagerState {
  if (!isRecord(value) || (value.version !== 1 && value.version !== 2) || !Array.isArray(value.batches)) {
    throw new Error("unsupported download state");
  }
  const version = value.version;
  return {
    version: 2,
    concurrency: clampConcurrency(Number(value.concurrency)),
    globallyPaused: Boolean(value.globallyPaused),
    batches: value.batches.map((batch) => readBatch(batch, version)).filter((batch) => batch.items.length > 0),
  };
}

export function loadDownloadState(storage: Storage): { state: DownloadManagerState; warning?: string } {
  const raw = storage.getItem(DOWNLOAD_STORAGE_KEY);
  if (!raw) return { state: createInitialState() };
  try {
    return { state: parseState(JSON.parse(raw)) };
  } catch {
    try {
      storage.setItem(`${CORRUPT_KEY_PREFIX}${Date.now()}`, raw);
    } catch {
      // The current session can still continue with an empty queue.
    }
    return { state: createInitialState(), warning: "下载记录损坏，已备份原数据并创建空队列。" };
  }
}

export function saveDownloadState(storage: Storage, state: DownloadManagerState) {
  storage.setItem(DOWNLOAD_STORAGE_KEY, JSON.stringify(state));
}
