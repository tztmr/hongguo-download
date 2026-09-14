import type { DefinitionPreference, EpisodeItem, SeriesItem } from "../types";

export type DownloadItemStatus = "queued" | "running" | "done" | "error";
export type DownloadBatchStatus = "running" | "queued" | "paused" | "done" | "error";

export type DownloadItem = {
  id: string;
  itemId: string;
  episodeIndex: number;
  episodeTitle: string;
  definition: string;
  status: DownloadItemStatus;
  percent: number;
  received: number;
  total?: number;
  path?: string;
  error?: string;
  addedAt: number;
  completedAt?: number;
};

export type DownloadSeriesSnapshot = Pick<
  SeriesItem,
  "bookId" | "seriesId" | "title" | "cover" | "abstract" | "category" | "categoryTags" | "contentTypeCode"
> & Partial<Pick<
  SeriesItem,
  "episodeCount" | "durationSeconds" | "author" | "onlineTime" | "releaseType"
>>;

export type DownloadBatch = {
  id: string;
  bookId: string;
  title: string;
  cover: string;
  series: DownloadSeriesSnapshot;
  paused: boolean;
  items: DownloadItem[];
  createdAt: number;
  updatedAt: number;
  removeWhenIdle?: boolean;
  completionNotifiedAt?: number;
};

export type DownloadManagerState = {
  version: 2;
  concurrency: number;
  globallyPaused: boolean;
  batches: DownloadBatch[];
};

export type DownloadStats = {
  running: number;
  queued: number;
  done: number;
  error: number;
};

export type DownloadAction =
  | { type: "set-concurrency"; value: number }
  | { type: "pause-all" }
  | { type: "resume-all" }
  | { type: "pause-batch"; batchId: string }
  | { type: "resume-batch"; batchId: string }
  | { type: "mark-running"; itemId: string }
  | { type: "update-progress"; itemId: string; received: number; total?: number; percent: number }
  | {
      type: "mark-done";
      itemId: string;
      path: string;
      definition: string;
      bytes: number;
      completedAt: number;
    }
  | { type: "mark-error"; itemId: string; error: string }
  | { type: "retry-item"; itemId: string }
  | { type: "retry-batch"; batchId: string }
  | { type: "remove-item"; itemId: string }
  | { type: "remove-batch"; batchId: string }
  | { type: "mark-batch-notified"; batchId: string; notifiedAt: number }
  | { type: "clear-completed" };

const defaultIdFactory = () => `${Date.now()}-${Math.random().toString(16).slice(2)}`;

export function clampConcurrency(value: number) {
  return Math.min(10, Math.max(1, Math.round(Number.isFinite(value) ? value : 8)));
}

export function createInitialState(): DownloadManagerState {
  return { version: 2, concurrency: 8, globallyPaused: false, batches: [] };
}

export function enqueueEpisodes(
  state: DownloadManagerState,
  series: SeriesItem,
  episodes: EpisodeItem[],
  definition: DefinitionPreference,
  idFactory: () => string = defaultIdFactory,
  now = Date.now(),
): { state: DownloadManagerState; added: number; skipped: number } {
  const existingBatchIndex = state.batches.findIndex((batch) => batch.bookId === series.bookId);
  const existingIds = new Set(
    state.batches
      .filter((batch) => batch.bookId === series.bookId)
      .flatMap((batch) => batch.items.map((item) => item.itemId)),
  );
  const freshEpisodes = episodes.filter((episode) => {
    if (existingIds.has(episode.itemId)) return false;
    existingIds.add(episode.itemId);
    return true;
  });
  if (!freshEpisodes.length) {
    return { state, added: 0, skipped: episodes.length };
  }

  const newItems: DownloadItem[] = freshEpisodes.map((episode) => ({
    id: idFactory(),
    itemId: episode.itemId,
    episodeIndex: episode.index,
    episodeTitle: episode.title,
    definition,
    status: "queued",
    percent: 0,
    received: 0,
    addedAt: now,
  }));
  let batches: DownloadBatch[];
  if (existingBatchIndex >= 0) {
    batches = state.batches.map((batch, index) =>
      index === existingBatchIndex
        ? { ...batch, items: [...batch.items, ...newItems], updatedAt: now, removeWhenIdle: false, completionNotifiedAt: undefined }
        : batch,
    );
  } else {
    batches = [
      ...state.batches,
      {
        id: idFactory(),
        bookId: series.bookId,
        title: series.title,
        cover: series.cover,
        series: {
          bookId: series.bookId,
          seriesId: series.seriesId,
          title: series.title,
          cover: series.cover,
          abstract: series.abstract,
          category: series.category,
          categoryTags: series.categoryTags,
          contentTypeCode: series.contentTypeCode,
          episodeCount: series.episodeCount,
          durationSeconds: series.durationSeconds,
          author: series.author,
          onlineTime: series.onlineTime,
          releaseType: series.releaseType,
        },
        paused: false,
        items: newItems,
        createdAt: now,
        updatedAt: now,
      },
    ];
  }
  return { state: { ...state, batches }, added: newItems.length, skipped: episodes.length - newItems.length };
}

function updateItem(
  state: DownloadManagerState,
  itemId: string,
  update: (item: DownloadItem) => DownloadItem,
): DownloadManagerState {
  const batchIndex = state.batches.findIndex(batch => batch.items.some(item => item.id === itemId));
  if (batchIndex < 0) return state;
  const batch = state.batches[batchIndex];
  const itemIndex = batch.items.findIndex(item => item.id === itemId);
  const item = update(batch.items[itemIndex]);
  if (item === batch.items[itemIndex]) return state;
  const items = batch.items.slice();
  items[itemIndex] = item;
  const batches = state.batches.slice();
  batches[batchIndex] = { ...batch, items, updatedAt: Date.now() };
  return { ...state, batches };
}

function removeIdleRequestedBatches(state: DownloadManagerState) {
  return {
    ...state,
    batches: state.batches.filter(
      (batch) => !(batch.removeWhenIdle && batch.items.every((item) => item.status !== "running")),
    ),
  };
}

export function downloadReducer(state: DownloadManagerState, action: DownloadAction): DownloadManagerState {
  switch (action.type) {
    case "set-concurrency":
      return { ...state, concurrency: clampConcurrency(action.value) };
    case "pause-all":
      return { ...state, globallyPaused: true };
    case "resume-all":
      return { ...state, globallyPaused: false };
    case "pause-batch":
    case "resume-batch":
      return {
        ...state,
        batches: state.batches.map((batch) =>
          batch.id === action.batchId ? { ...batch, paused: action.type === "pause-batch", updatedAt: Date.now() } : batch,
        ),
      };
    case "mark-running":
      return updateItem(state, action.itemId, (item) =>
        item.status === "queued" ? { ...item, status: "running", error: undefined } : item,
      );
    case "update-progress":
      // Progress and command results use separate IPC paths. A late event must
      // never revive a settled download or take a queued retry out of the queue.
      return updateItem(state, action.itemId, (item) =>
        item.status === "running" && (item.received !== action.received || item.total !== action.total || item.percent !== Math.max(0, Math.min(100, action.percent)))
          ? {
              ...item,
              received: action.received,
              total: action.total,
              percent: Math.max(0, Math.min(100, action.percent)),
            }
          : item,
      );
    case "mark-done":
      return removeIdleRequestedBatches(
        updateItem(state, action.itemId, (item) => ({
          ...item,
          status: "done",
          percent: 100,
          received: action.bytes,
          total: action.bytes,
          path: action.path,
          definition: action.definition,
          completedAt: action.completedAt,
          error: undefined,
        })),
      );
    case "mark-error":
      return removeIdleRequestedBatches(
        updateItem(state, action.itemId, (item) => ({ ...item, status: "error", error: action.error })),
      );
    case "retry-item":
      return updateItem(state, action.itemId, (item) =>
        item.status === "error"
          ? { ...item, status: "queued", percent: 0, received: 0, total: undefined, error: undefined }
          : item,
      );
    case "retry-batch":
      return {
        ...state,
        batches: state.batches.map((batch) =>
          batch.id === action.batchId
            ? {
                ...batch,
                paused: false,
                removeWhenIdle: false,
                updatedAt: Date.now(),
                items: batch.items.map((item) =>
                  item.status === "error"
                    ? { ...item, status: "queued", percent: 0, received: 0, total: undefined, error: undefined }
                    : item,
                ),
              }
            : batch,
        ),
      };
    case "remove-item": {
      const batches = state.batches
        .map((batch) => ({
          ...batch,
          items: batch.items.filter((item) => item.id !== action.itemId || item.status === "running"),
        }))
        .filter((batch) => batch.items.length > 0);
      return { ...state, batches };
    }
    case "remove-batch": {
      const batch = state.batches.find((item) => item.id === action.batchId);
      if (!batch) return state;
      if (batch.items.some((item) => item.status === "running")) {
        return {
          ...state,
          batches: state.batches.map((item) =>
            item.id === action.batchId ? { ...item, paused: true, removeWhenIdle: true, updatedAt: Date.now() } : item,
          ),
        };
      }
      return { ...state, batches: state.batches.filter((item) => item.id !== action.batchId) };
    }
    case "mark-batch-notified":
      return {
        ...state,
        batches: state.batches.map((batch) =>
          batch.id === action.batchId && batch.completionNotifiedAt === undefined
            ? { ...batch, completionNotifiedAt: action.notifiedAt, updatedAt: Math.max(batch.updatedAt, action.notifiedAt) }
            : batch,
        ),
      };
    case "clear-completed":
      return { ...state, batches: state.batches.filter((batch) => !batch.items.every((item) => item.status === "done")) };
  }
}

export function deriveBatchStatus(batch: DownloadBatch): DownloadBatchStatus {
  if (batch.items.some((item) => item.status === "running")) return "running";
  if (batch.paused && batch.items.some((item) => item.status === "queued")) return "paused";
  if (batch.items.some((item) => item.status === "queued")) return "queued";
  if (batch.items.some((item) => item.status === "error")) return "error";
  return "done";
}

export function getDownloadStats(state: DownloadManagerState): DownloadStats {
  const stats: DownloadStats = { running: 0, queued: 0, done: 0, error: 0 };
  for (const item of state.batches.flatMap((batch) => batch.items)) {
    stats[item.status] += 1;
  }
  return stats;
}

export function selectLaunchableItems(
  state: DownloadManagerState,
  activeIds: ReadonlySet<string>,
  limit: number,
  cursor: number,
): { items: Array<{ batchId: string; item: DownloadItem }>; cursor: number } {
  if (state.globallyPaused || limit <= 0 || !state.batches.length) return { items: [], cursor };
  const selected: Array<{ batchId: string; item: DownloadItem }> = [];
  const reserved = new Set(activeIds);
  let nextCursor = ((cursor % state.batches.length) + state.batches.length) % state.batches.length;

  while (selected.length < limit) {
    let found = false;
    for (let offset = 0; offset < state.batches.length; offset += 1) {
      const index = (nextCursor + offset) % state.batches.length;
      const batch = state.batches[index];
      if (batch.paused || batch.removeWhenIdle) continue;
      const item = batch.items.find((candidate) => candidate.status === "queued" && !reserved.has(candidate.id));
      if (!item) continue;
      selected.push({ batchId: batch.id, item });
      reserved.add(item.id);
      nextCursor = (index + 1) % state.batches.length;
      found = true;
      break;
    }
    if (!found) break;
  }
  return { items: selected, cursor: nextCursor };
}
