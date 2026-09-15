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
  MediaScheduling,
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
  const [scheduling, setScheduling] = useState<MediaScheduling>();
  const [jobs, setJobs] = useState<MediaJob[]>(initialJobs);
  const [warning, setWarning] = useState<string | undefined>(initialWarning);
  const [error, setError] = useState<MediaCommandError | undefined>();
  const commandsRef = useRef(commands);
  commandsRef.current = commands;
  const revision = useRef(0);
  const jobRevisions = useRef(new Map<string, number>());
  const removedJobIds = useRef(new Set<string>());

  const applyJob = useCallback((job: MediaJob, before = Infinity) => {
    if (removedJobIds.current.has(job.id) || (jobRevisions.current.get(job.id) || 0) > before) return;
    jobRevisions.current.set(job.id, ++revision.current);
    setJobs(current => upsertJob(current, job));
  }, []);

  useEffect(() => {
    if (!enabled) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    const before = revision.current;
    void (async () => {
      try {
        const nextUnlisten = await commandsRef.current.subscribeProgress((job) => {
          if (active) applyJob(job);
        });
        if (!active) { nextUnlisten(); return; }
        unlisten = nextUnlisten;
      } catch (nextError) {
        if (active) setError(asMediaError(nextError));
      }
      if (!active) return;
      try {
        const snapshot = await commandsRef.current.snapshot();
        if (!active) return;
        setJobs(current => {
          let next = snapshot.jobs.filter(job => !removedJobIds.current.has(job.id));
          // Events and actions completed during the request are newer than its snapshot.
          for (const job of current) {
            if (!removedJobIds.current.has(job.id) && (jobRevisions.current.get(job.id) || 0) > before) next = upsertJob(next, job);
          }
          return next;
        });
        setWarning(snapshot.warning?.message);
      } catch (nextError) {
        if (active) setError(asMediaError(nextError));
      }
    })();
    return () => {
      active = false;
      unlisten?.();
    };
  }, [enabled, applyJob]);

  useEffect(() => {
    if (!enabled || !commandsRef.current.scheduling) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      try { const value = await commandsRef.current.scheduling!(); if (active) setScheduling(value); }
      catch { /* Keep job controls available if the status probe fails. */ }
      finally { if (active) timer = setTimeout(refresh, 2000); }
    }
    void refresh();
    return () => { active = false; clearTimeout(timer); };
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
      const before = revision.current;
      try {
        const job = await commandsRef.current.startMerge({
          bookId: batch.bookId,
          title: batch.title,
          seriesRoot: seriesRootFromInputs(inputs),
          outputFileName: options.outputName,
          inputs,
          transcodeH264: options.transcodeH264,
          mode: options.mode,
          quality: options.quality,
          conflictPolicy: options.conflictPolicy,
        });
        applyJob(job, before);
        setError(undefined);
        return job;
      } catch (nextError) {
        const normalized = asMediaError(nextError);
        setError(normalized);
        throw normalized;
      }
    },
    [applyJob],
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

  const pause = useCallback(async (jobId: string) => {
    const before = revision.current;
    try {
      const job = await commandsRef.current.pause(jobId);
      applyJob(job, before);
      setError(undefined);
    } catch (nextError) {
      const normalized = asMediaError(nextError);
      setError(normalized);
      throw normalized;
    }
  }, [applyJob]);

  const resume = useCallback(async (jobId: string) => {
    const before = revision.current;
    try {
      const job = await commandsRef.current.resume(jobId);
      applyJob(job, before);
      setError(undefined);
    } catch (nextError) {
      const normalized = asMediaError(nextError);
      setError(normalized);
      throw normalized;
    }
  }, [applyJob]);

  const deleteJob = useCallback(async (jobId: string) => {
    try {
      await commandsRef.current.deleteJob(jobId);
      removedJobIds.current.add(jobId);
      setJobs((current) => current.filter((job) => job.id !== jobId));
      setError(undefined);
    } catch (nextError) {
      const normalized = asMediaError(nextError);
      setError(normalized);
      throw normalized;
    }
  }, []);

  const hasMergedVideo = useCallback(async (seriesRoot: string) => {
    try {
      return await commandsRef.current.hasMergedVideo(seriesRoot);
    } catch {
      return false;
    }
  }, []);

  const findMergedVideo = useCallback(async (seriesRoot: string) => {
    try {
      return await commandsRef.current.findMergedVideo(seriesRoot);
    } catch {
      return null;
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
    const before = revision.current;
    try {
      const job = await commandsRef.current[command]({
        bookId: batch.bookId,
        title: batch.title,
        seriesRoot: seriesRootFromInputs(episodeInputs),
        scope,
        inputs,
        model,
      });
      applyJob(job, before);
      setError(undefined);
      return job;
    } catch (nextError) {
      const normalized = asMediaError(nextError);
      setError(normalized);
      throw normalized;
    }
  }, [applyJob]);

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
    const before = revision.current;
    try {
      const job = await commandsRef.current.retry(jobId);
      applyJob(job, before);
      setError(undefined);
    } catch (nextError) {
      const normalized = asMediaError(nextError);
      setError(normalized);
      throw normalized;
    }
  }, [applyJob]);

  const markNotified = useCallback(async (jobId: string, outcome: "success" | "failure") => {
    if (!commandsRef.current.markNotified) return;
    const before = revision.current;
    const job = await commandsRef.current.markNotified(jobId, outcome);
    applyJob(job, before);
  }, [applyJob]);

  return useMemo(
    () => ({ jobs, scheduling, warning, error, startMerge, startAudioSeparation, startSubtitleExtraction, cancel, pause, resume, deleteJob, hasMergedVideo, findMergedVideo, retry, markNotified }),
    [jobs, scheduling, warning, error, startMerge, startAudioSeparation, startSubtitleExtraction, cancel, pause, resume, deleteJob, hasMergedVideo, findMergedVideo, retry, markNotified],
  );
}
