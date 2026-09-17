import { useEffect, useRef, useState } from "react";
import { Cover } from "../components/Cover";
import type { ManagedVideo, ManagementCommands, VideoAiDraft } from "./managementCommands";

type TextTarget = Pick<ManagedVideo, "title" | "description" | "privacyStatus">;
type DraftRow = {
  video: ManagedVideo; title: string; description: string; image: string;
  textReady: boolean; coverReady: boolean; textSaved: boolean; coverSaved: boolean;
  refreshRequired: boolean; unconfirmedText: TextTarget | null; error: string;
};
const privacyLabels = { private: "私人", unlisted: "不公开", public: "公开" };
function message(error: unknown) {
  return error && typeof error === "object" && "message" in error ? String(error.message) : typeof error === "string" ? error : "操作失败，请刷新核对后重试";
}
function errorCode(error: unknown) {
  return error && typeof error === "object" && "code" in error ? String(error.code) : "";
}
function shouldStop(error: unknown, mode: "generate" | "save") {
  const code = errorCode(error);
  if (mode === "generate") return !["YOUTUBE_VIDEO_CHANGED", "YOUTUBE_VIDEO_NOT_FOUND", "YOUTUBE_INVALID_METADATA", "AI_INVALID_CONTENT", "AI_INVALID_IMAGE", "AUTOMATION_COVER_INVALID"].includes(code);
  // A single video's upload, conversion or readback failure must not abandon
  // the rest of the batch. Only known channel-wide blockers pause syncing.
  return ["AUTH_REQUIRED", "OAUTH_CLIENT_REJECTED", "OAUTH_REFRESH_TOKEN_MISSING", "YOUTUBE_CHANNEL_CHANGED", "YOUTUBE_CHANNEL_MISMATCH", "YOUTUBE_CHANNEL_NOT_FOUND", "YOUTUBE_QUOTA_EXCEEDED", "THUMBNAIL_RATE_LIMITED"].includes(code);
}
function sameText(actual: TextTarget, expected: TextTarget) {
  return actual.title === expected.title && actual.description === expected.description && actual.privacyStatus === expected.privacyStatus;
}
function validateDraft(draft: VideoAiDraft, video: ManagedVideo) {
  if (draft.video.id !== video.id || draft.video.etag !== video.etag) throw new Error("视频资料已变化，请关闭并刷新列表后重新生成");
}
function validText(title: string, description: string) {
  return !!title.trim() && [...title.trim()].length <= 100 && new TextEncoder().encode(description).length <= 5000 && !/[<>]/.test(title + description);
}

export function VideoAiDialog({ videos, channelId, channelTitle, commands, onUpdated, onClose }: {
  videos: ManagedVideo[]; channelId: string; channelTitle: string; commands: ManagementCommands;
  onUpdated(video: ManagedVideo): void; onClose(): void;
}) {
  const [options, setOptions] = useState({ title: true, description: true, cover: true });
  const [rows, setRows] = useState<DraftRow[]>(() => videos.map(video => ({ video, title: video.title, description: video.description, image: "", textReady: false, coverReady: false, textSaved: false, coverSaved: false, refreshRequired: false, unconfirmedText: null, error: "" })));
  const rowsRef = useRef(rows);
  const [busy, setBusy] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const [saveAttempted, setSaveAttempted] = useState(false);
  const [progress, setProgress] = useState("");
  const [stopReason, setStopReason] = useState("");
  const [confirmClose, setConfirmClose] = useState(false);
  const busyRef = useRef(false), alive = useRef(true), stopRef = useRef(false);
  const dialogRef = useRef<HTMLDivElement>(null);
  const wantsText = options.title || options.description;
  const needsGeneration = (row: DraftRow) => (wantsText && !row.textReady) || (options.cover && !row.coverReady);
  const pendingGeneration = rows.some(needsGeneration);
  const pendingSave = (row: DraftRow) => (wantsText && row.textReady && !row.textSaved) || (options.cover && row.coverReady && !row.coverSaved);
  const invalid = (row: DraftRow) => wantsText && row.textReady && !row.textSaved && !validText(options.title ? row.title : row.video.title, options.description ? row.description : row.video.description);
  const pendingSync = rows.filter(row => pendingSave(row) || row.refreshRequired);
  const synced = wantsText || options.cover ? rows.filter(row => (!wantsText || row.textSaved) && (!options.cover || row.coverSaved) && !row.refreshRequired).length : 0;
  const syncSummary = [
    wantsText && `文案已同步 ${rows.filter(row => row.textSaved).length} / ${videos.length}`,
    options.cover && `封面已上传 ${rows.filter(row => row.coverSaved).length} / ${videos.length}`,
    rows.some(row => row.refreshRequired) && `待核对 ${rows.filter(row => row.refreshRequired).length} 个`,
  ].filter(Boolean).join(" · ");
  function targetText(row: DraftRow): TextTarget {
    return { title: options.title ? row.title.trim() : row.video.title, description: options.description ? row.description : row.video.description, privacyStatus: row.video.privacyStatus };
  }
  useEffect(() => {
    alive.current = true;
    const previous = document.activeElement as HTMLElement | null;
    dialogRef.current?.focus();
    return () => { alive.current = false; stopRef.current = true; previous?.focus(); };
  }, []);
  function patch(id: string, value: Partial<DraftRow>) {
    rowsRef.current = rowsRef.current.map(row => row.video.id === id ? { ...row, ...value } : row);
    if (alive.current) setRows(rowsRef.current);
  }
  function current(id: string) { return rowsRef.current.find(row => row.video.id === id)!; }
  function close() {
    if (busyRef.current) return;
    if (rowsRef.current.some(row => pendingSave(row) || row.refreshRequired)) setConfirmClose(true); else onClose();
  }
  async function refresh(id: string) {
    const row = current(id);
    const video = await commands.detail(channelId, id);
    if (!alive.current) return;
    if (video.id !== id) throw new Error("返回的视频不匹配，请刷新核对后重试");
    // A write can succeed even when its response/readback fails. Reconcile the
    // original or requested state before reusing a fresh revision on retry.
    if (!sameText(video, row.video) && !(row.unconfirmedText && sameText(video, row.unconfirmedText))) {
      throw { code: "YOUTUBE_VIDEO_CHANGED", message: "视频标题、说明或可见性已被其他操作修改，请关闭并刷新列表后核对；未覆盖最新资料" };
    }
    patch(id, { video, refreshRequired: false, unconfirmedText: null, textSaved: row.textReady && wantsText && sameText(video, targetText(row)) }); onUpdated(video);
  }
  async function run(mode: "generate" | "save") {
    if (busyRef.current || (!wantsText && !options.cover) || (mode === "save" && rowsRef.current.some(invalid))) return;
    busyRef.current = true; stopRef.current = false; setBusy(true); setAttempted(true); setStopReason(""); setConfirmClose(false);
    if (mode === "save") setSaveAttempted(true);
    try {
      for (const [index, initial] of rowsRef.current.entries()) {
        if (!alive.current || stopRef.current) break;
        if (mode === "generate" ? !needsGeneration(initial) : !pendingSave(initial) && !initial.refreshRequired) continue;
        const id = initial.video.id;
        patch(id, { error: "" });
        setProgress(`${mode === "generate" ? "生成" : "同步"} ${index + 1} / ${videos.length}：${initial.video.title}`);
        let stage = mode === "generate" ? "生成失败" : "读取最新资料失败";
        try {
          if (current(id).refreshRequired) await refresh(id);
          if (!alive.current || stopRef.current) break;
          let row = current(id);
          if (mode === "generate") {
            if (wantsText && !row.textReady) {
              const draft = await commands.generateAi(channelId, id, row.video.etag, "text");
              if (!alive.current) break;
              validateDraft(draft, row.video);
              if (!draft.title?.trim() || !draft.description?.trim()) throw new Error("AI 未返回完整文案，请重试生成");
              patch(id, { title: draft.title, description: draft.description, textReady: true });
            }
            if (stopRef.current) break;
            row = current(id);
            if (options.cover && !row.coverReady) {
              const draft = await commands.generateAi(channelId, id, row.video.etag, "cover");
              if (!alive.current) break;
              validateDraft(draft, row.video);
              if (!draft.image || !/^data:image\/(png|jpeg|webp);base64,/.test(draft.image)) throw new Error("AI 未返回有效封面，请重试生成");
              patch(id, { image: draft.image, coverReady: true });
            }
          } else {
            if (wantsText && row.textReady && !row.textSaved) {
              stage = "文案同步失败";
              const target = targetText(row);
              patch(id, { refreshRequired: true, unconfirmedText: target });
              const updated = await commands.update({ channelId, videoId: id, etag: row.video.etag, ...target });
              if (!alive.current) break;
              if (updated.id !== id) throw new Error("返回的视频不匹配，请刷新核对后重试");
              if (!sameText(updated, target)) throw new Error("YouTube 尚未确认文案修改，请核对后重试");
              patch(id, { video: updated, textSaved: true, refreshRequired: false, unconfirmedText: null }); onUpdated(updated);
            }
            if (stopRef.current) break;
            row = current(id);
            if (options.cover && row.coverReady && !row.coverSaved) {
              stage = "封面上传失败";
              patch(id, { refreshRequired: true });
              await commands.generatedThumbnail(channelId, id, row.video.etag, row.image);
              if (!alive.current) break;
              // Remember the accepted upload before readback, so retry cannot upload it twice.
              patch(id, { coverSaved: true, refreshRequired: true });
              try { await refresh(id); }
              catch (error) {
                stage = "封面已上传，读取最新资料失败";
                throw { code: errorCode(error), message: `重试仅刷新资料，不会重复上传封面。${message(error)}` };
              }
            }
          }
        } catch (error) {
          if (!alive.current) break;
          patch(id, { error: `${stage}：${message(error)}` });
          if (shouldStop(error, mode)) { setStopReason(message(error)); break; }
        }
      }
    } finally {
      busyRef.current = false;
      if (alive.current) { setBusy(false); setProgress(""); if (stopRef.current) setStopReason("已按要求停止后续操作"); }
    }
  }
  return <div className="dialog-backdrop"><div ref={dialogRef} tabIndex={-1} className="yt-edit-dialog yt-ai-dialog" role="dialog" aria-modal="true" aria-labelledby="yt-ai-title" onKeyDown={event => {
    if (event.key === "Escape") { event.stopPropagation(); close(); }
    if (event.key === "Tab") {
      const controls = [...event.currentTarget.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled), textarea:not(:disabled)")];
      const first = controls[0], last = controls[controls.length - 1];
      if (!first) { event.preventDefault(); return; }
      if (event.shiftKey && (document.activeElement === first || document.activeElement === event.currentTarget)) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && (document.activeElement === last || document.activeElement === event.currentTarget)) { event.preventDefault(); first.focus(); }
    }
  }}>
    <header><div><h2 id="yt-ai-title">AI 一键优化视频</h2><p>{channelTitle || channelId} · {videos.length} 个视频 / Shorts</p></div><span>已同步 {synced} / {videos.length}</span></header>
    <div className="yt-editor-body">
      <div className="yt-ai-intro"><p>使用自动追剧中已保存的 AI 服务、模型与提示词。生成会使用对应服务额度，预览满意后再同步到 YouTube。</p>
        <div className="yt-ai-options">{(["title", "description", "cover"] as const).map(key => <label key={key}><input type="checkbox" checked={options[key]} disabled={busy || attempted} onChange={event => setOptions({ ...options, [key]: event.target.checked })} />{{ title: "标题", description: "说明", cover: "封面" }[key]}</label>)}</div>
      </div>
      {stopReason && <p role="status" className="warning-banner">批量操作已暂停：{stopReason}。生成和同步进度已保留，重试只处理未完成的部分。</p>}
      {!busy && saveAttempted && synced < videos.length && <p role="status" className="warning-banner">还有 {videos.length - synced} 个视频未完成。{pendingGeneration ? "请先生成未完成项，再同步剩余内容。" : "可重试未完成项，已同步文案和已上传封面不会重复提交。"}</p>}
      {rows.map(row => <section className="yt-ai-card" key={row.video.id} aria-label={`优化 ${row.video.id}`}>
        <div className="yt-ai-card-heading"><div><h3>{row.video.title}</h3><small>{row.video.id} · {privacyLabels[row.video.privacyStatus]}</small></div><span>{wantsText && `文案${row.textSaved ? "已同步" : row.textReady ? "待同步" : "待生成"}`}{wantsText && options.cover ? " · " : ""}{options.cover && `封面${row.coverSaved ? "已上传" : row.coverReady ? "待同步" : "待生成"}`}{row.refreshRequired && " · 待核对"}</span></div>
        <div className={`yt-ai-draft${options.cover ? "" : " yt-ai-text-only"}`}>
          {options.cover && <figure><Cover src={row.image || row.video.thumbnailUrl} title={row.coverReady ? "AI 封面预览" : "当前视频封面"} className="yt-ai-cover" /><figcaption>{row.coverReady ? "AI 封面预览" : "当前视频封面"}</figcaption></figure>}
          {wantsText && <div className="yt-editor-fields">
            {options.title && <label>标题<input aria-label={`AI 标题 ${row.video.id}`} value={row.title} disabled={busy || !row.textReady || row.textSaved} onChange={event => patch(row.video.id, { title: event.target.value })} /><small>{[...row.title].length} / 100 字符</small></label>}
            {options.description && <label>说明<textarea aria-label={`AI 说明 ${row.video.id}`} value={row.description} disabled={busy || !row.textReady || row.textSaved} onChange={event => patch(row.video.id, { description: event.target.value })} /><small>{new TextEncoder().encode(row.description).length} / 5000 字节</small></label>}
            {invalid(row) && <p className="error-copy" role="alert">标题需为 1～100 字符，说明最多 5000 字节，均不可包含 &lt; 或 &gt;。</p>}
          </div>}
        </div>
        {row.error && <p className="error-copy" role="alert">{row.error}</p>}
      </section>)}
    </div>
    <footer>
      {confirmClose ? <><span>关闭将丢弃尚未同步的预览。</span><button type="button" className="secondary-button" onClick={() => setConfirmClose(false)}>继续编辑</button><button type="button" className="secondary-button" onClick={onClose}>放弃预览并关闭</button></>
        : <><span role="status">{progress || (synced === videos.length ? "所选内容已全部同步" : saveAttempted ? syncSummary : "先生成预览，再同步修改")}</span><div className="yt-ai-footer-actions">
          <button type="button" className="secondary-button" disabled={busy} onClick={close}>关闭</button>
          {busy ? <button type="button" className="secondary-button" onClick={() => { stopRef.current = true; }}>停止后续</button> : <>
            <button type="button" className="secondary-button" disabled={!pendingGeneration} onClick={() => void run("generate")}>{attempted ? "生成未完成项" : "一键生成"}</button>
            <button type="button" className="primary-button compact" disabled={rows.some(invalid) || !pendingSync.length} onClick={() => void run("save")}>{saveAttempted && pendingSync.length ? `重试未完成项（${pendingSync.length}）` : "一键同步到 YouTube"}</button>
          </>}
        </div></>}
    </footer>
  </div></div>;
}
