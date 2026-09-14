import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { DefinitionPreference, EpisodeItem, SeriesItem } from "../types";
import {
  downloadReducer,
  enqueueEpisodes,
  getDownloadStats,
  selectLaunchableItems,
  type DownloadAction,
  type DownloadBatch,
  type DownloadManagerState,
} from "./model";
import { loadDownloadState, saveDownloadState } from "./storage";

export type DownloadProgress = {
  taskId: string;
  received: number;
  total?: number;
  percent: number;
};

export type DownloadAdapter = {
  download(args: {
    taskId: string;
    itemId: string;
    title: string;
    episodeTitle: string;
    definition: string;
    series: DownloadBatch["series"];
  }): Promise<{ taskId: string; path: string; definition: string; bytes: number }>;
  subscribeProgress(listener: (progress: DownloadProgress) => void): Promise<() => void>;
};

type UseDownloadManagerOptions = {
  adapter: DownloadAdapter;
  storage: Storage;
  initialState?: DownloadManagerState;
  enabled?: boolean;
  definition?: DefinitionPreference;
  onBatchCompleted?: (batch: DownloadBatch) => Promise<void> | void;
};

const MAX_AUTO_RETRIES = 3;
const AUTO_RETRY_DELAYS_MS = [1_000, 3_000, 8_000] as const;

function isRetryableDownloadError(error: string) {
  return !/(?:HTTP\s*(?:400|404|422)|内容过小|视频解密失败|item_id.*无效)/i.test(error);
}

export function useDownloadManager({ adapter, storage, initialState, enabled = true, definition = "auto", onBatchCompleted }: UseDownloadManagerOptions) {
  const [loaded] = useState(() => (initialState ? { state: initialState } : loadDownloadState(storage)));
  const [state, setState] = useState(loaded.state);
  const [warning, setWarning] = useState<string | undefined>(loaded.warning);
  const [schedulerTick, setSchedulerTick] = useState(0);
  const stateRef = useRef(state);
  const mountedRef = useRef(true);
  const activeRef = useRef(new Map<string, Promise<void>>());
  const cursorRef = useRef(0);
  const retryAttemptsRef = useRef(new Map<string, number>());
  const retryTimersRef = useRef(new Map<string, number>());
  const persistTimerRef = useRef<number | undefined>(undefined);
  const completionClaimedRef = useRef(new Set<string>());
  const onBatchCompletedRef = useRef(onBatchCompleted);
  onBatchCompletedRef.current = onBatchCompleted;

  const persist = useCallback(
    (next: DownloadManagerState, immediate: boolean) => {
      const write = () => {
        try {
          saveDownloadState(storage, next);
          setWarning((current) => (current?.startsWith("无法保存") ? undefined : current));
        } catch (error) {
          setWarning(`无法保存下载记录：${error instanceof Error ? error.message : String(error)}`);
        }
      };
      if (persistTimerRef.current !== undefined) window.clearTimeout(persistTimerRef.current);
      if (immediate) {
        write();
      } else {
        persistTimerRef.current = window.setTimeout(write, 500);
      }
    },
    [storage],
  );

  const replaceState = useCallback(
    (next: DownloadManagerState, immediate = true) => {
      if (next === stateRef.current) return;
      stateRef.current = next;
      setState(next);
      persist(next, immediate);
    },
    [persist],
  );

  const commit = useCallback(
    (action: DownloadAction, immediate = action.type !== "update-progress") => {
      replaceState(downloadReducer(stateRef.current, action), immediate);
    },
    [replaceState],
  );

  const clearRetryTimer = useCallback((itemId: string) => {
    const timer = retryTimersRef.current.get(itemId);
    if (timer !== undefined) {
      clearTimeout(timer);
      retryTimersRef.current.delete(itemId);
    }
  }, []);

  const scheduleAutomaticRetry = useCallback((itemId: string, error: string) => {
    const previousAttempts = retryAttemptsRef.current.get(itemId) || 0;
    const nextAttempt = previousAttempts + 1;
    if (nextAttempt > MAX_AUTO_RETRIES || !isRetryableDownloadError(error)) {
      retryAttemptsRef.current.delete(itemId);
      commit({ type: "mark-error", itemId, error });
      return;
    }

    retryAttemptsRef.current.set(itemId, nextAttempt);
    commit({
      type: "mark-error",
      itemId,
      error: `自动重试中（第 ${nextAttempt}/${MAX_AUTO_RETRIES} 次）：${error}`,
    });
    clearRetryTimer(itemId);
    const timer = setTimeout(() => {
      retryTimersRef.current.delete(itemId);
      if (!mountedRef.current) return;
      const current = stateRef.current.batches
        .flatMap((batch) => batch.items)
        .find((item) => item.id === itemId);
      if (!current || current.status !== "error") {
        retryAttemptsRef.current.delete(itemId);
        return;
      }
      commit({ type: "retry-item", itemId });
    }, AUTO_RETRY_DELAYS_MS[nextAttempt - 1]);
    retryTimersRef.current.set(itemId, timer);
  }, [clearRetryTimer, commit]);

  useEffect(() => {
    if (!enabled) return;
    mountedRef.current = true;
    let unsubscribe: (() => void) | undefined;
    void adapter.subscribeProgress((progress) => {
      if (!mountedRef.current) return;
      // Automation uses the same native event but owns a separate durable queue.
      // Ignore it before scanning/serializing the manual download history.
      if (!activeRef.current.has(progress.taskId)) return;
      commit({
        type: "update-progress",
        itemId: progress.taskId,
        received: progress.received,
        total: progress.total,
        percent: progress.percent,
      });
    }).then((nextUnsubscribe) => {
      if (!mountedRef.current) nextUnsubscribe();
      else unsubscribe = nextUnsubscribe;
    });
    return () => {
      mountedRef.current = false;
      unsubscribe?.();
      for (const timer of retryTimersRef.current.values()) clearTimeout(timer);
      retryTimersRef.current.clear();
      if (persistTimerRef.current !== undefined) window.clearTimeout(persistTimerRef.current);
    };
  }, [adapter, commit, enabled]);

  useEffect(() => {
    if (!enabled) return;
    const available = state.concurrency - activeRef.current.size;
    if (available <= 0) return;
    const selected = selectLaunchableItems(state, new Set(activeRef.current.keys()), available, cursorRef.current);
    cursorRef.current = selected.cursor;
    for (const { batchId, item } of selected.items) {
      const batch = state.batches.find((candidate) => candidate.id === batchId);
      if (!batch || activeRef.current.has(item.id)) continue;
      activeRef.current.set(item.id, Promise.resolve());
      commit({ type: "mark-running", itemId: item.id });
      const operation = adapter
        .download({
          taskId: item.id,
          itemId: item.itemId,
          title: batch.title,
          episodeTitle: item.episodeTitle,
          definition: item.definition,
          series: batch.series,
        })
        .then((result) => {
          if (!mountedRef.current) return;
          retryAttemptsRef.current.delete(item.id);
          commit({
            type: "mark-done",
            itemId: item.id,
            path: result.path,
            definition: result.definition,
            bytes: result.bytes,
            completedAt: Date.now(),
          });
        })
        .catch((error) => {
          if (!mountedRef.current) return;
          scheduleAutomaticRetry(item.id, error instanceof Error ? error.message : String(error));
        })
        .finally(() => {
          activeRef.current.delete(item.id);
          if (mountedRef.current) setSchedulerTick((value) => value + 1);
        });
      activeRef.current.set(item.id, operation);
    }
  }, [adapter, commit, enabled, schedulerTick, scheduleAutomaticRetry, state]);

  const enqueue = useCallback(
    (series: SeriesItem, episodes: EpisodeItem[]) => {
      const result = enqueueEpisodes(stateRef.current, series, episodes, definition);
      if (result.state !== stateRef.current) replaceState(result.state);
      return { added: result.added, skipped: result.skipped };
    },
    [definition, replaceState],
  );

  const stats = useMemo(() => getDownloadStats(state), [state]);

  const retryItem = useCallback((itemId: string) => {
    clearRetryTimer(itemId);
    retryAttemptsRef.current.delete(itemId);
    commit({ type: "retry-item", itemId });
  }, [clearRetryTimer, commit]);

  const retryBatch = useCallback((batchId: string) => {
    const batch = stateRef.current.batches.find((candidate) => candidate.id === batchId);
    for (const item of batch?.items || []) {
      if (item.status === "error") {
        clearRetryTimer(item.id);
        retryAttemptsRef.current.delete(item.id);
      }
    }
    commit({ type: "retry-batch", batchId });
  }, [clearRetryTimer, commit]);

  useEffect(() => {
    if (!enabled) return;
    for (const batch of state.batches) {
      if (
        batch.completionNotifiedAt !== undefined ||
        completionClaimedRef.current.has(batch.id) ||
        !batch.items.length ||
        !batch.items.every((item) => item.status === "done")
      ) continue;
      completionClaimedRef.current.add(batch.id);
      try {
        void Promise.resolve(onBatchCompletedRef.current?.(batch)).catch(() => undefined);
      } catch {
        // Notification callbacks are best effort and never block queue state.
      }
      commit({ type: "mark-batch-notified", batchId: batch.id, notifiedAt: Date.now() });
    }
  }, [commit, enabled, state]);

  return {
    state,
    stats,
    warning,
    enqueue,
    pauseAll: () => commit({ type: "pause-all" }),
    resumeAll: () => commit({ type: "resume-all" }),
    setConcurrency: (value: number) => commit({ type: "set-concurrency", value }),
    pauseBatch: (batchId: string) => commit({ type: "pause-batch", batchId }),
    resumeBatch: (batchId: string) => commit({ type: "resume-batch", batchId }),
    retryItem,
    retryBatch,
    removeItem: (itemId: string) => commit({ type: "remove-item", itemId }),
    removeBatch: (batchId: string) => commit({ type: "remove-batch", batchId }),
    clearCompleted: () => commit({ type: "clear-completed" }),
  };
}

export type DownloadManager = ReturnType<typeof useDownloadManager>;
