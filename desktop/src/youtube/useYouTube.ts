import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { youtubeCommands } from "./commands";
import type { YouTubeCommands, YouTubeError, YouTubeJob, YouTubeModel, YouTubeSnapshot, YouTubeUploadIntent } from "./types";

const EMPTY: YouTubeSnapshot = {
  credential: { configured: false, clientIdSuffix: "" },
  channels: [],
  activeChannelId: null,
  jobs: [],
};

function normalizedError(error: unknown): YouTubeError {
  const visit = (value: unknown): YouTubeError | undefined => {
    if (!value || typeof value !== "object") return undefined;
    const record = value as Record<string, unknown>;
    if (typeof record.code === "string") {
      return { code: record.code, message: typeof record.message === "string" ? record.message : "YouTube 操作失败" };
    }
    return visit(record.payload) || visit(record.error) || visit(record.data);
  };
  return visit(error) || { code: "UNKNOWN_ERROR", message: "YouTube 操作失败，请重试" };
}

function upsert(jobs: YouTubeJob[], next: YouTubeJob) {
  const index = jobs.findIndex((job) => job.id === next.id);
  if (index < 0) return [...jobs, next];
  const copy = jobs.slice();
  copy[index] = next;
  return copy;
}

export function useYouTube(commands: YouTubeCommands = youtubeCommands, enabled = true): YouTubeModel {
  const [snapshot, setSnapshot] = useState(EMPTY);
  const [loading, setLoading] = useState(enabled);
  const [busy, setBusy] = useState(false);
  const pendingActions = useRef(0);
  const [error, setError] = useState<YouTubeError>();
  const jobEvents = useRef(new Map<string, number>());
  const removedJobIds = useRef(new Set<string>());
  const commandsRef = useRef(commands);
  commandsRef.current = commands;

  const refresh = useCallback((next: YouTubeSnapshot, before: ReadonlyMap<string, number>) => {
    setSnapshot(current => {
      let jobs = next.jobs.filter(job => !removedJobIds.current.has(job.id));
      for (const job of current.jobs) {
        if (!removedJobIds.current.has(job.id) && (jobEvents.current.get(job.id) || 0) !== (before.get(job.id) || 0)) jobs = upsert(jobs, job);
      }
      return { ...next, jobs };
    });
  }, []);

  useEffect(() => {
    if (!enabled) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    const before = new Map(jobEvents.current);
    setLoading(true);
    void (async () => {
      try {
        const value = await commandsRef.current.subscribeProgress((job) => {
          if (active && !removedJobIds.current.has(job.id)) {
            jobEvents.current.set(job.id, (jobEvents.current.get(job.id) || 0) + 1);
            setSnapshot((current) => ({ ...current, jobs: upsert(current.jobs, job) }));
          }
        });
        if (!active) { value(); return; }
        unlisten = value;
      } catch (next) { if (active) setError(normalizedError(next)); }
      if (!active) return;
      try {
        const next = await commandsRef.current.snapshot();
        if (active) refresh(next, before);
      } catch (next) { if (active) setError(normalizedError(next)); }
      finally { if (active) setLoading(false); }
    })();
    return () => { active = false; unlisten?.(); };
  }, [enabled, refresh]);

  const action = useCallback(async <T,>(operation: () => Promise<T>, apply?: (value: T) => void) => {
    pendingActions.current += 1;
    setBusy(true);
    setError(undefined);
    try {
      const value = await operation();
      apply?.(value);
      setError(undefined);
      return value;
    } catch (next) {
      const safe = normalizedError(next);
      setError(safe);
      throw safe;
    } finally {
      pendingActions.current -= 1;
      setBusy(pendingActions.current > 0);
    }
  }, []);

  const snapshotAction = useCallback((operation: () => Promise<YouTubeSnapshot>) => {
    const before = new Map(jobEvents.current);
    return action(operation, next => refresh(next, before));
  }, [action, refresh]);
  const importCredential = useCallback(async (path: string) => {
    await action(() => commandsRef.current.importCredential(path), (credential) => setSnapshot((current) => ({ ...current, credential })));
  }, [action]);
  const authorize = useCallback(async () => {
    await action(() => commandsRef.current.authorize(), (channel) => setSnapshot((current) => ({
      ...current,
      channels: [...current.channels.filter((item) => item.channelId !== channel.channelId), channel],
      activeChannelId: channel.channelId,
    })));
  }, [action]);
  const setChannel = useCallback(async (channelId: string) => { await snapshotAction(() => commandsRef.current.setChannel(channelId)); }, [snapshotAction]);
  const revoke = useCallback(async (channelId: string) => { await snapshotAction(() => commandsRef.current.revoke(channelId)); }, [snapshotAction]);
  const removeCredential = useCallback(async () => { await snapshotAction(() => commandsRef.current.removeCredential()); }, [snapshotAction]);
  const jobAction = useCallback((jobId: string, operation: () => Promise<YouTubeJob>) => {
    const before = jobEvents.current.get(jobId) || 0;
    return action(operation, (job) => {
      // Progress can arrive before an IPC reply (e.g. Paused before Pausing).
      if (!removedJobIds.current.has(jobId) && (jobEvents.current.get(jobId) || 0) === before) {
        jobEvents.current.set(jobId, before + 1);
        setSnapshot((current) => ({ ...current, jobs: upsert(current.jobs, job) }));
      }
    });
  }, [action]);
  const startUpload = useCallback((request: YouTubeUploadIntent) => {
    removedJobIds.current.delete(request.jobId);
    return jobAction(request.jobId, () => commandsRef.current.startUpload(request));
  }, [jobAction]);
  const cancel = useCallback(async (jobId: string) => { await action(() => commandsRef.current.cancel(jobId)); }, [action]);
  const pause = useCallback(async (jobId: string) => { await jobAction(jobId, () => commandsRef.current.pause(jobId)); }, [jobAction]);
  const resume = useCallback(async (jobId: string) => { await jobAction(jobId, () => commandsRef.current.resume(jobId)); }, [jobAction]);
  const retry = useCallback(async (jobId: string) => { await jobAction(jobId, () => commandsRef.current.retry(jobId)); }, [jobAction]);
  const retryThumbnail = useCallback(async (jobId: string) => { await jobAction(jobId, () => commandsRef.current.retryThumbnail(jobId)); }, [jobAction]);
  const uploadSubtitle = useCallback(async (jobId: string, request: YouTubeUploadIntent["subtitle"]) => { await jobAction(jobId, () => commandsRef.current.uploadSubtitle(jobId, request)); }, [jobAction]);
  const removeJob = useCallback(async (jobId: string) => {
    await action(() => commandsRef.current.removeJob(jobId), () => {
      removedJobIds.current.add(jobId);
      setSnapshot((current) => ({ ...current, jobs: current.jobs.filter((job) => job.id !== jobId) }));
    });
  }, [action]);
  const markNotified = useCallback(async (jobId: string, outcome: "success" | "failure") => {
    if (!commandsRef.current.markNotified) return;
    await jobAction(jobId, () => commandsRef.current.markNotified!(jobId, outcome));
  }, [jobAction]);

  return useMemo(() => ({ ...snapshot, loading, busy, error, importCredential, authorize, setChannel, revoke, removeCredential, startUpload, cancel, pause, resume, retry, retryThumbnail, uploadSubtitle, removeJob, markNotified }), [snapshot, loading, busy, error, importCredential, authorize, setChannel, revoke, removeCredential, startUpload, cancel, pause, resume, retry, retryThumbnail, uploadSubtitle, removeJob, markNotified]);
}
