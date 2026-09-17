import { useEffect, useRef, useState } from "react";
import type { ManagedVideo, ManagementCommands } from "./managementCommands";
import type { YouTubePrivacy } from "./types";

function errorMessage(error: unknown) {
  return error && typeof error === "object" && "message" in error ? String(error.message) : typeof error === "string" ? error : "操作结果未确认，请刷新核对后重试";
}
export type VideoBulkAction = "delete" | "private" | "visibility";
export function VideoBulkActionDialog({ action, videos, skippedPrivate = 0, channelId, channelTitle, commands, onDeleted, onUpdated, onClose }: {
  action: VideoBulkAction; skippedPrivate?: number;
  videos: ManagedVideo[]; channelId: string; channelTitle: string; commands: ManagementCommands;
  onDeleted(id: string): void; onUpdated(video: ManagedVideo): void; onClose(): void;
}) {
  const deleting = action === "delete";
  const [targetPrivacy, setTargetPrivacy] = useState<YouTubePrivacy>("private");
  const labels = { private: "私人", unlisted: "不公开", public: "公开" };
  const expectedPrivacy = action === "private" ? "private" : targetPrivacy;
  const verb = deleting ? "删除" : `设为${labels[expectedPrivacy]}`;
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
  async function submit() {
    if (busyRef.current || !remaining.length) return;
    busyRef.current = true; setBusy(true); setAttempted(true); setFailures({});
    const pending = [...remaining];
    try {
      for (let index = 0; index < pending.length; index++) {
        if (!alive.current) break;
        const video = pending[index];
        setProgress(`正在${verb} ${index + 1} / ${pending.length}：${video.title}`);
        try {
          if (deleting) {
            await commands.deleteVideo(channelId, video.id);
            if (!alive.current) break;
            onDeleted(video.id);
          } else {
            const updated = action === "private" ? await commands.makeBlockedVideoPrivate(channelId, video.id)
              : await commands.setPrivacy(channelId, video.id, targetPrivacy);
            if (!alive.current) break;
            if (updated.id !== video.id) throw new Error("返回的视频不匹配，请刷新核对后重试");
            onUpdated(updated);
            if (updated.privacyStatus !== expectedPrivacy) throw new Error(`YouTube 尚未确认${verb}，请刷新核对后重试`);
          }
          if (!alive.current) break;
          setRemaining((rows) => rows.filter((row) => row.id !== video.id));
        } catch (error) {
          if (!alive.current) break;
          const detail = errorMessage(error);
          const code = error && typeof error === "object" && "code" in error ? String(error.code) : "";
          // A quota, auth or transport problem affects subsequent requests too.
          const stop = !code || ["AUTH_REQUIRED", "YOUTUBE_QUOTA_EXCEEDED", "YOUTUBE_CHANNEL_CHANGED", "YOUTUBE_CHANNEL_MISMATCH", "YOUTUBE_MANAGEMENT_FORBIDDEN", "YOUTUBE_MANAGEMENT_NETWORK"].includes(code);
          setFailures((old) => ({ ...old, ...Object.fromEntries((stop ? pending.slice(index) : [video]).map((row) => [row.id, row.id === video.id ? detail : `尚未执行：${detail}`])) }));
          if (stop) break;
        }
      }
    } finally {
      busyRef.current = false;
      if (alive.current) { setBusy(false); setProgress(""); }
    }
  }
  return <div className="dialog-backdrop"><div ref={dialogRef} tabIndex={-1} className="yt-edit-dialog yt-delete-dialog" role={deleting ? "alertdialog" : "dialog"} aria-modal="true" aria-labelledby="yt-bulk-title" aria-describedby="yt-bulk-description" onKeyDown={(event) => {
    if (event.key === "Escape") { event.stopPropagation(); if (!busyRef.current) onClose(); }
    if (event.key === "Tab") {
      const controls = Array.from(event.currentTarget.querySelectorAll<HTMLElement>("button:not(:disabled), select:not(:disabled)"));
      const first = controls[0], last = controls[controls.length - 1];
      if (!first) { event.preventDefault(); return; }
      if (event.shiftKey && (document.activeElement === first || document.activeElement === event.currentTarget)) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && (document.activeElement === last || document.activeElement === event.currentTarget)) { event.preventDefault(); first.focus(); }
    }
  }}>
    <header><h2 id="yt-bulk-title">{deleting ? "批量删除 YouTube 视频" : action === "private" ? "封锁视频批量设为私人" : "批量更改视频状态"}</h2></header>
    <div className="yt-editor-body">
      <p id="yt-bulk-description">频道：{channelTitle || channelId}。{deleting ? "将永久删除以下视频及 Shorts，无法恢复。请核对标题和 ID。" : action === "private" ? "将以下封锁视频及 Shorts 的可见性改为私人，仅你和你指定的人可观看。此操作不会解除版权或地区限制。" : "按所选状态更改以下视频及 Shorts 的可见性。设为私人会同时取消已安排的定时发布。"}</p>
      {action === "visibility" && <label className="yt-privacy-target">目标可见性<select aria-label="批量目标可见性" value={targetPrivacy} disabled={busy || attempted} onChange={event => setTargetPrivacy(event.target.value as YouTubePrivacy)}>{Object.entries(labels).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>}
      <p>共选择 {videos.length} 个 · 已{verb} {videos.length - remaining.length} 个 · 剩余 {remaining.length} 个</p>
      {skippedPrivate > 0 && <p>已跳过 {skippedPrivate} 个私人封锁视频。</p>}
      {Object.keys(failures).length > 0 && <div role="alert" className="warning-banner">部分视频未{verb}，原因见下方。重试只处理剩余视频；结果不确定时可先关闭并刷新核对。</div>}
      {remaining.length ? <ul className="yt-delete-targets">{remaining.map((video) => <li key={video.id}><strong>{video.title}</strong><small>{video.id} · {video.restriction.reason}</small>{failures[video.id] && <p className="error-copy">{failures[video.id]}</p>}</li>)}</ul> : <div className="yt-save-notice" role="status">所选视频已全部{verb}</div>}
      {progress && <p role="status">{progress}</p>}
    </div>
    <footer><button className="secondary-button" type="button" disabled={busy} onClick={onClose}>{attempted ? "关闭" : "取消"}</button>
      {remaining.length > 0 && <button className={deleting ? "secondary-button danger" : "primary-button compact"} type="button" disabled={busy} onClick={() => void submit()}>{busy ? `正在${verb}…` : attempted ? `重试剩余 ${remaining.length} 个` : `确认${deleting ? "永久删除" : verb} ${remaining.length} 个`}</button>}
    </footer>
  </div></div>;
}
