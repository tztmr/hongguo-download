import { useEffect, useRef, useState } from "react";
import { Cover } from "../components/Cover";
import type { ManagedVideo, ManagementCommands, VideoAiDraft } from "./managementCommands";

type DraftRow = {
  video: ManagedVideo; title: string; description: string; image: string;
  textReady: boolean; coverReady: boolean; textSaved: boolean; coverSaved: boolean;
  refreshRequired: boolean; error: string;
};
const privacyLabels = { private: "私人", unlisted: "不公开", public: "公开" };
function message(error: unknown) {
  return error && typeof error === "object" && "message" in error ? String(error.message) : typeof error === "string" ? error : "操作失败，请刷新核对后重试";
}
function shouldStop(error: unknown) {
  const code = error && typeof error === "object" && "code" in error ? String(error.code) : "";
  return !["YOUTUBE_VIDEO_CHANGED", "YOUTUBE_VIDEO_NOT_FOUND", "YOUTUBE_INVALID_METADATA", "AI_INVALID_CONTENT", "AI_INVALID_IMAGE", "AUTOMATION_COVER_INVALID"].includes(code);
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
  const [rows, setRows] = useState<DraftRow[]>(() => videos.map(video => ({ video, title: video.title, description: video.description, image: "", textReady: false, coverReady: false, textSaved: false, coverSaved: false, refreshRequired: false, error: "" })));
  const rowsRef = useRef(rows);
  const [busy, setBusy] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const [progress, setProgress] = useState("");
  const [stopped, setStopped] = useState(false);
  const [confirmClose, setConfirmClose] = useState(false);
  const busyRef = useRef(false), alive = useRef(true), stopRef = useRef(false);
  const dialogRef = useRef<HTMLDivElement>(null);
  const wantsText = options.title || options.description;
  const pendingGeneration = rows.some(row => (wantsText && !row.textReady) || (options.cover && !row.coverReady));
  const pendingSave = (row: DraftRow) => (wantsText && row.textReady && !row.textSaved) || (options.cover && row.coverReady && !row.coverSaved);
  const invalid = (row: DraftRow) => wantsText && row.textReady && !row.textSaved && !validText(options.title ? row.title : row.video.title, options.description ? row.description : row.video.description);
  const synced = wantsText || options.cover ? rows.filter(row => (!wantsText || row.textSaved) && (!options.cover || row.coverSaved)).length : 0;
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
    if (rowsRef.current.some(pendingSave)) setConfirmClose(true); else onClose();
  }
  async function refresh(id: string) {
    const video = await commands.detail(channelId, id);
    if (!alive.current) return;
    if (video.id !== id) throw new Error("返回的视频不匹配，请刷新核对后重试");
    patch(id, { video, refreshRequired: false }); onUpdated(video);
  }
  async function run(mode: "generate" | "save") {
    if (busyRef.current || (!wantsText && !options.cover) || (mode === "save" && rowsRef.current.some(invalid))) return;
    busyRef.current = true; stopRef.current = false; setBusy(true); setAttempted(true); setStopped(false); setConfirmClose(false);
    try {
      for (const [index, initial] of rowsRef.current.entries()) {
        if (!alive.current || stopRef.current) break;
        const id = initial.video.id;
        patch(id, { error: "" });
        setProgress(`${mode === "generate" ? "生成" : "同步"} ${index + 1} / ${videos.length}：${initial.video.title}`);
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
              const title = options.title ? row.title.trim() : row.video.title;
              const description = options.description ? row.description : row.video.description;
              const updated = await commands.update({ channelId, videoId: id, etag: row.video.etag, title, description, privacyStatus: row.video.privacyStatus });
              if (!alive.current) break;
              if (updated.id !== id) throw new Error("返回的视频不匹配，请刷新核对后重试");
              patch(id, { video: updated }); onUpdated(updated);
              if (updated.title !== title || updated.description !== description) throw new Error("YouTube 尚未确认文案修改，请核对后重试");
              patch(id, { textSaved: true });
            }
            if (stopRef.current) break;
            row = current(id);
            if (options.cover && row.coverReady && !row.coverSaved) {
              await commands.generatedThumbnail(channelId, id, row.video.etag, row.image);
              if (!alive.current) break;
              // Remember the accepted upload before readback, so retry cannot upload it twice.
              patch(id, { coverSaved: true, refreshRequired: true });
              try { await refresh(id); }
              catch { throw new Error("封面已上传，读取最新资料失败；点击同步重试仅刷新资料，不会重复上传封面"); }
            }
          }
        } catch (error) {
          if (!alive.current) break;
          patch(id, { error: message(error) });
          if (shouldStop(error)) { setStopped(true); break; }
        }
      }
    } finally {
      busyRef.current = false;
      if (alive.current) { setBusy(false); setProgress(""); if (stopRef.current) setStopped(true); }
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
      {stopped && <p role="status" className="warning-banner">已停止后续操作，生成和同步进度已保留。重试只处理未完成的部分。</p>}
      {rows.map(row => <section className="yt-ai-card" key={row.video.id} aria-label={`优化 ${row.video.id}`}>
        <div className="yt-ai-card-heading"><div><h3>{row.video.title}</h3><small>{row.video.id} · {privacyLabels[row.video.privacyStatus]}</small></div><span>{wantsText && `文案${row.textSaved ? "已同步" : row.textReady ? "待同步" : "待生成"}`}{wantsText && options.cover ? " · " : ""}{options.cover && `封面${row.coverSaved ? "已上传" : row.coverReady ? "待同步" : "待生成"}`}</span></div>
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
        : <><span role="status">{progress || (synced === videos.length && !rows.some(row => row.refreshRequired) ? "所选内容已全部同步" : "先生成预览，再同步修改")}</span><div className="yt-ai-footer-actions">
          <button type="button" className="secondary-button" disabled={busy} onClick={close}>关闭</button>
          {busy ? <button type="button" className="secondary-button" onClick={() => { stopRef.current = true; }}>停止后续</button> : <>
            <button type="button" className="secondary-button" disabled={!pendingGeneration} onClick={() => void run("generate")}>{attempted ? "生成未完成项" : "一键生成"}</button>
            <button type="button" className="primary-button compact" disabled={rows.some(invalid) || !rows.some(row => pendingSave(row) || row.refreshRequired)} onClick={() => void run("save")}>一键同步到 YouTube</button>
          </>}
        </div></>}
    </footer>
  </div></div>;
}
