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
  const [error, setError] = useState<YouTubeError>();
  const commandsRef = useRef(commands);
  commandsRef.current = commands;

  useEffect(() => {
    if (!enabled) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    void commandsRef.current.snapshot().then((next) => {
      if (active) setSnapshot(next);
    }).catch((next) => {
      if (active) setError(normalizedError(next));
    }).finally(() => {
      if (active) setLoading(false);
    });
    void commandsRef.current.subscribeProgress((job) => {
      if (active) setSnapshot((current) => ({ ...current, jobs: upsert(current.jobs, job) }));
    }).then((value) => {
      if (active) unlisten = value;
      else value();
    }).catch((next) => {
      if (active) setError(normalizedError(next));
    });
    return () => { active = false; unlisten?.(); };
  }, [enabled]);

  const action = useCallback(async <T,>(operation: () => Promise<T>, apply?: (value: T) => void) => {
    setBusy(true);
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
      setBusy(false);
    }
  }, []);

  const refresh = useCallback((next: YouTubeSnapshot) => setSnapshot(next), []);
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
  const setChannel = useCallback(async (channelId: string) => { await action(() => commandsRef.current.setChannel(channelId), refresh); }, [action, refresh]);
  const revoke = useCallback(async (channelId: string) => { await action(() => commandsRef.current.revoke(channelId), refresh); }, [action, refresh]);
  const removeCredential = useCallback(async () => { await action(() => commandsRef.current.removeCredential(), refresh); }, [action, refresh]);
  const startUpload = useCallback((request: YouTubeUploadIntent) => action(
    () => commandsRef.current.startUpload(request),
    (job) => setSnapshot((current) => ({ ...current, jobs: upsert(current.jobs, job) })),
  ), [action]);
  const cancel = useCallback(async (jobId: string) => { await action(() => commandsRef.current.cancel(jobId)); }, [action]);
  const retry = useCallback(async (jobId: string) => { await action(() => commandsRef.current.retry(jobId), (job) => setSnapshot((current) => ({ ...current, jobs: upsert(current.jobs, job) }))); }, [action]);
  const retryThumbnail = useCallback(async (jobId: string) => { await action(() => commandsRef.current.retryThumbnail(jobId), (job) => setSnapshot((current) => ({ ...current, jobs: upsert(current.jobs, job) }))); }, [action]);
  const markNotified = useCallback(async (jobId: string, outcome: "success" | "failure") => {
    if (!commandsRef.current.markNotified) return;
    await action(() => commandsRef.current.markNotified!(jobId, outcome), (next) => setSnapshot((current) => ({ ...current, jobs: upsert(current.jobs, next) })));
  }, [action]);

  return useMemo(() => ({ ...snapshot, loading, busy, error, importCredential, authorize, setChannel, revoke, removeCredential, startUpload, cancel, retry, retryThumbnail, markNotified }), [snapshot, loading, busy, error, importCredential, authorize, setChannel, revoke, removeCredential, startUpload, cancel, retry, retryThumbnail, markNotified]);
}
