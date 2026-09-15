import { useEffect, useRef, useState } from "react";
import type { ManagedVideo, ManagementCommands } from "./managementCommands";

function errorMessage(error: unknown) {
  return error && typeof error === "object" && "message" in error ? String(error.message) : "删除结果未确认，请刷新核对后重试";
}
export function VideoDeleteDialog({ videos, channelId, channelTitle, commands, onDeleted, onClose }: {
  videos: ManagedVideo[]; channelId: string; channelTitle: string; commands: ManagementCommands;
  onDeleted(id: string): void; onClose(): void;
}) {
  const [remaining, setRemaining] = useState(videos);
  const [failures, setFailures] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const [progress, setProgress] = useState("");
  const busyRef = useRef(false);
  const alive = useRef(true);
  const dialogRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    alive.current = true;
    const previous = document.activeElement as HTMLElement | null;
    dialogRef.current?.focus();
    return () => { alive.current = false; previous?.focus(); };
  }, []);
  async function remove() {
    if (busyRef.current || !remaining.length) return;
    busyRef.current = true; setBusy(true); setAttempted(true); setFailures({});
    const pending = [...remaining];
    try {
      for (let index = 0; index < pending.length; index++) {
        if (!alive.current) break;
        const video = pending[index];
        setProgress(`正在删除 ${index + 1} / ${pending.length}：${video.title}`);
        try {
          await commands.deleteVideo(channelId, video.id);
          if (!alive.current) break;
          setRemaining((rows) => rows.filter((row) => row.id !== video.id));
          onDeleted(video.id);
        } catch (error) {
          if (!alive.current) break;
          const detail = errorMessage(error);
          const code = error && typeof error === "object" && "code" in error ? String(error.code) : "";
          // A quota, auth or transport problem affects subsequent requests too.
          const stop = !code || ["AUTH_REQUIRED", "YOUTUBE_QUOTA_EXCEEDED", "YOUTUBE_CHANNEL_CHANGED", "YOUTUBE_MANAGEMENT_NETWORK"].includes(code);
          setFailures((old) => ({ ...old, ...Object.fromEntries((stop ? pending.slice(index) : [video]).map((row) => [row.id, row.id === video.id ? detail : `尚未执行：${detail}`])) }));
          if (stop) break;
        }
      }
    } finally {
      busyRef.current = false;
      if (alive.current) { setBusy(false); setProgress(""); }
    }
  }
  return <div className="dialog-backdrop"><div ref={dialogRef} tabIndex={-1} className="yt-edit-dialog yt-delete-dialog" role="alertdialog" aria-modal="true" aria-labelledby="yt-delete-title" aria-describedby="yt-delete-description" onKeyDown={(event) => {
    if (event.key === "Escape") { event.stopPropagation(); if (!busyRef.current) onClose(); }
    if (event.key === "Tab") {
      const controls = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"));
      const first = controls[0], last = controls[controls.length - 1];
      if (!first) { event.preventDefault(); return; }
      if (event.shiftKey && (document.activeElement === first || document.activeElement === event.currentTarget)) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && (document.activeElement === last || document.activeElement === event.currentTarget)) { event.preventDefault(); first.focus(); }
    }
  }}>
    <header><h2 id="yt-delete-title">批量删除 YouTube 视频</h2></header>
    <div className="yt-editor-body">
      <p id="yt-delete-description">频道：{channelTitle || channelId}。将永久删除以下视频及 Shorts，无法恢复。请核对标题和 ID。</p>
      <p>共选择 {videos.length} 个 · 已删除 {videos.length - remaining.length} 个 · 剩余 {remaining.length} 个</p>
      {Object.keys(failures).length > 0 && <div role="alert" className="warning-banner">部分视频未删除，原因见下方。重试只处理剩余视频；结果不确定时可先关闭并刷新核对。</div>}
      {remaining.length ? <ul className="yt-delete-targets">{remaining.map((video) => <li key={video.id}><strong>{video.title}</strong><small>{video.id}</small>{failures[video.id] && <p className="error-copy">{failures[video.id]}</p>}</li>)}</ul> : <div className="yt-save-notice" role="status">所选视频已全部删除</div>}
      {progress && <p role="status">{progress}</p>}
    </div>
    <footer><button className="secondary-button" type="button" disabled={busy} onClick={onClose}>{attempted ? "关闭" : "取消"}</button>
      {remaining.length > 0 && <button className="secondary-button danger" type="button" disabled={busy} onClick={() => void remove()}>{busy ? "正在删除…" : attempted ? `重试剩余 ${remaining.length} 个` : `确认永久删除 ${remaining.length} 个`}</button>}
    </footer>
  </div></div>;
}
