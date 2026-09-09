import { useEffect, useRef, useState } from "react";
import { loadEpisodeVideo } from "../playback";
import type { DefinitionPreference, EpisodeItem } from "../types";

type OnlinePlayerProps = {
  title: string;
  episodes: EpisodeItem[];
  initialItemId: string;
  definition: DefinitionPreference;
  onClose: () => void;
};

export function OnlinePlayer({ title, episodes, initialItemId, definition, onClose }: OnlinePlayerProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const stopPlayback = useRef<() => void>(() => {});
  const [itemId, setItemId] = useState(initialItemId);
  const [attempt, setAttempt] = useState(0);
  const [source, setSource] = useState("");
  const [error, setError] = useState("");
  const [ready, setReady] = useState(false);
  const index = episodes.findIndex((episode) => episode.itemId === itemId);

  useEffect(() => {
    const dialog = dialogRef.current!;
    const previousFocus = document.activeElement as HTMLElement | null;
    if (typeof dialog.showModal === "function") dialog.showModal();
    else dialog.setAttribute("open", "");
    return () => {
      if (typeof dialog.close === "function") dialog.close();
      previousFocus?.focus();
    };
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    stopPlayback.current = () => controller.abort();
    let objectUrl = "";
    setSource("");
    setError("");
    setReady(false);
    const loadingTimeout = window.setTimeout(() => {
      controller.abort();
      setError("视频准备超时，请重试或降低清晰度");
    }, 300_000);
    void loadEpisodeVideo(itemId, definition, controller.signal).then((blob) => {
      if (controller.signal.aborted) return;
      window.clearTimeout(loadingTimeout);
      if ("mediaSource" in blob) blob.currentTime = () => videoRef.current?.currentTime ?? 0;
      objectUrl = URL.createObjectURL("mediaSource" in blob ? blob.mediaSource : blob);
      setSource(objectUrl);
      if ("mediaSource" in blob) void blob.finished.catch((reason) => {
        if (!controller.signal.aborted) {
          setError(reason instanceof Error ? reason.message : String(reason));
          controller.abort();
        }
      });
    }).catch((reason) => {
      window.clearTimeout(loadingTimeout);
      if (!controller.signal.aborted) setError(reason instanceof Error ? reason.message : String(reason));
    });
    return () => {
      window.clearTimeout(loadingTimeout);
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [itemId, definition, attempt]);

  useEffect(() => {
    if (!source || ready || error) return;
    const timeout = window.setTimeout(() => { setError("视频画面未能加载，请重试播放"); stopPlayback.current(); }, 30_000);
    return () => window.clearTimeout(timeout);
  }, [source, ready, error]);

  return (
    <dialog ref={dialogRef} className="online-player" aria-label="在线观看" onCancel={(event) => { event.preventDefault(); onClose(); }}>
      <header className="online-player-header">
        <div><h2>{title}</h2><p>{episodes[index]?.title} · {definition === "auto" ? "自动最高" : definition}</p></div>
        <button type="button" className="secondary-button" onClick={onClose} autoFocus>关闭播放</button>
      </header>
      <div className="online-player-screen">
        {error ? <div className="online-player-state"><p role="alert">{error}</p><button type="button" className="secondary-button" onClick={() => setAttempt((value) => value + 1)}>重试播放</button></div>
          : source ? <>
            <video ref={videoRef} key={source} src={source} preload="auto" controls autoPlay playsInline aria-label={episodes[index]?.title}
              onLoadedData={() => setReady(true)} onPlaying={() => setReady(true)}
              onError={(event) => { stopPlayback.current(); const code = event.currentTarget.error?.code; setError(code === 3 || code === 4 ? "视频画面解码失败，请重试或切换剧集" : "当前视频无法播放，请重试或切换剧集"); }} />
            {!ready ? <p className="online-player-state online-player-loading" role="status">正在加载视频画面…</p> : null}
            </> : <p className="online-player-state" role="status">正在获取视频…</p>}
      </div>
      <footer className="online-player-controls">
        <button type="button" className="secondary-button" disabled={index <= 0} onClick={() => setItemId(episodes[index - 1].itemId)}>上一集</button>
        <select aria-label="播放剧集" value={itemId} onChange={(event) => setItemId(event.target.value)}>
          {episodes.map((episode) => <option key={episode.itemId} value={episode.itemId}>{episode.title}</option>)}
        </select>
        <button type="button" className="secondary-button" disabled={index < 0 || index >= episodes.length - 1} onClick={() => setItemId(episodes[index + 1].itemId)}>下一集</button>
      </footer>
    </dialog>
  );
}
