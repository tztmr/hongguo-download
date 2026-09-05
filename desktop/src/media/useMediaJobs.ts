import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { DownloadBatch } from "../download/model";
import { mediaCommands } from "./commands";
import { asMediaError } from "./errors";
import { completedMergeInputs, isCompletedBatch, seriesRootFromInputs } from "./paths";
import type {
  MediaCommandError,
  MediaCommands,
  MediaJob,
  MediaJobsModel,
  MediaJobScope,
  MergeSubmitOptions,
} from "./types";

export type UseMediaJobsOptions = {
  commands?: MediaCommands;
  enabled?: boolean;
  initialJobs?: MediaJob[];
  initialWarning?: string;
};

function upsertJob(jobs: MediaJob[], next: MediaJob) {
  const index = jobs.findIndex((job) => job.id === next.id);
  if (index < 0) return [...jobs, next];
  const copy = jobs.slice();
  copy[index] = next;
  return copy;
}

export function useMediaJobs({
  commands = mediaCommands,
  enabled = true,
  initialJobs = [],
  initialWarning,
}: UseMediaJobsOptions = {}): MediaJobsModel {
  const [jobs, setJobs] = useState<MediaJob[]>(initialJobs);
  const [warning, setWarning] = useState<string | undefined>(initialWarning);
  const [error, setError] = useState<MediaCommandError | undefined>();
  const commandsRef = useRef(commands);
  commandsRef.current = commands;

  useEffect(() => {
    if (!enabled) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const snapshot = await commandsRef.current.snapshot();
        if (!active) return;
        setJobs(snapshot.jobs);
        setWarning(snapshot.warning?.message);
      } catch (nextError) {
        if (active) setError(asMediaError(nextError));
      }
      try {
        const nextUnlisten = await commandsRef.current.subscribeProgress((job) => {
          if (!active) return;
          setJobs((current) => upsertJob(current, job));
        });
        if (!active) {
          nextUnlisten();
          return;
        }
        unlisten = nextUnlisten;
      } catch (nextError) {
        if (active) setError(asMediaError(nextError));
      }
    })();
    return () => {
      active = false;
      unlisten?.();
    };
  }, [enabled]);

  const startMerge = useCallback(
    async (batch: DownloadBatch, options: MergeSubmitOptions) => {
      if (!isCompletedBatch(batch)) {
        const normalized = {
          code: "MERGE_INPUT_INVALID",
          message: "全部剧集下载完成后才能合并",
        };
        setError(normalized);
        throw normalized;
      }
      const inputs = completedMergeInputs(batch);
      try {
        const job = await commandsRef.current.startMerge({
          bookId: batch.bookId,
          title: batch.title,
          seriesRoot: seriesRootFromInputs(inputs),
          outputFileName: options.outputName,
          inputs,
          transcodeH264: options.transcodeH264,
          conflictPolicy: options.conflictPolicy,
        });
        setJobs((current) => upsertJob(current, job));
        setError(undefined);
        return job;
      } catch (nextError) {
        const normalized = asMediaError(nextError);
        setError(normalized);
        throw normalized;
      }
    },
    [],
  );

  const cancel = useCallback(async (jobId: string) => {
    try {
      await commandsRef.current.cancel(jobId);
      setError(undefined);
    } catch (nextError) {
      const normalized = asMediaError(nextError);
      setError(normalized);
      throw normalized;
    }
  }, []);

  const startAI = useCallback(async (
    batch: DownloadBatch,
    scope: MediaJobScope,
    model: "htdemucs" | "htdemucs_ft" | "small" | "medium",
    command: "startAudioSeparation" | "startSubtitleExtraction",
    mergedPath?: string,
  ) => {
    if (!isCompletedBatch(batch)) {
      const normalized = { code: "AI_INPUT_INVALID", message: "全部剧集下载完成后才能进行 AI 媒体处理" };
      setError(normalized);
      throw normalized;
    }
    const episodeInputs = completedMergeInputs(batch);
    if (scope === "merged" && !mergedPath) {
      const normalized = { code: "MEDIA_SCOPE_INVALID", message: "当前剧目没有可用的合并视频" };
      setError(normalized);
      throw normalized;
    }
    const inputs = scope === "merged"
      ? [{ episodeIndex: 1, path: mergedPath as string }]
      : episodeInputs;
    try {
      const job = await commandsRef.current[command]({
        bookId: batch.bookId,
        title: batch.title,
        seriesRoot: seriesRootFromInputs(episodeInputs),
        scope,
        inputs,
        model,
      });
      setJobs((current) => upsertJob(current, job));
      setError(undefined);
      return job;
    } catch (nextError) {
      const normalized = asMediaError(nextError);
      setError(normalized);
      throw normalized;
    }
  }, []);

  const startAudioSeparation = useCallback(
    (batch: DownloadBatch, scope: MediaJobScope, model: "htdemucs" | "htdemucs_ft", mergedPath?: string) =>
      startAI(batch, scope, model, "startAudioSeparation", mergedPath),
    [startAI],
  );

  const startSubtitleExtraction = useCallback(
    (batch: DownloadBatch, scope: MediaJobScope, model: "small" | "medium", mergedPath?: string) =>
      startAI(batch, scope, model, "startSubtitleExtraction", mergedPath),
    [startAI],
  );

  const retry = useCallback(async (jobId: string) => {
    try {
      const job = await commandsRef.current.retry(jobId);
      setJobs((current) => upsertJob(current, job));
      setError(undefined);
    } catch (nextError) {
      const normalized = asMediaError(nextError);
      setError(normalized);
      throw normalized;
    }
  }, []);

  const markNotified = useCallback(async (jobId: string, outcome: "success" | "failure") => {
    if (!commandsRef.current.markNotified) return;
    const job = await commandsRef.current.markNotified(jobId, outcome);
    setJobs((current) => upsertJob(current, job));
  }, []);

  return useMemo(
    () => ({ jobs, warning, error, startMerge, startAudioSeparation, startSubtitleExtraction, cancel, retry, markNotified }),
    [jobs, warning, error, startMerge, startAudioSeparation, startSubtitleExtraction, cancel, retry, markNotified],
  );
}
