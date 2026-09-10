import { SubtitleUploadDialog } from "./SubtitleUploadDialog";
import { useEffect, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import type { YouTubeJob, YouTubeModel } from "./types";

function statusCopy(job: YouTubeJob) {
  return {
    queued: "排队中",
    pausing: "正在暂停",
    paused: "已暂停",
    preparingAuthorization: "准备授权",
    creatingSession: "创建上传会话",
    uploading: "上传中",
    waitingToRetry: "等待重试",
    processing: "YouTube 处理中",
    settingThumbnail: "设置封面",
    uploadingSubtitles: "上传字幕",
    videoUploadedSubtitleFailed: "视频已上传，字幕失败",
    completed: "已完成",
    videoUploadedThumbnailFailed: "视频已上传，封面失败",
    failed: "失败",
    cancelled: "已取消",
  }[job.status];
}

export function YouTubeVideoLink({ url, label = "打开 YouTube 视频" }: { url: string; label?: string }) {
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState("");
  return <>
    <a href={url} target="_blank" rel="noreferrer" aria-disabled={opening} onClick={async (event) => {
      if (!isTauri()) return;
      // Handle the desktop launch explicitly, including errors. Prevent the
      // WebView/plugin's delegated link handler from opening it a second time.
      event.preventDefault();
      if (opening) return;
      setOpening(true);
      setError("");
      try {
        await invoke("plugin:opener|open_url", { url });
      } catch (reason) {
        const detail = reason instanceof Error ? reason.message : typeof reason === "string" ? reason : "请检查默认浏览器设置";
        setError(`无法打开 YouTube 视频：${detail}`);
      } finally {
        setOpening(false);
      }
    }}>{opening ? "正在打开…" : label}</a>
    {error ? <small className="error-copy" role="alert">{error}</small> : null}
  </>;
}

const activeStatuses = ["pausing", "preparingAuthorization", "creatingSession", "uploading", "waitingToRetry", "processing", "settingThumbnail", "uploadingSubtitles"];
const attentionStatuses = ["failed", "videoUploadedThumbnailFailed", "videoUploadedSubtitleFailed"];
function fileSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
  return bytes >= 1024 ** 3 ? `${(bytes / 1024 ** 3).toFixed(2)} GB` : `${(bytes / 1024 ** 2).toFixed(1)} MB`;
}

function actionErrorMessage(error: unknown): string {
  return error && typeof error === "object" && "message" in error ? String(error.message)
    : typeof error === "string" ? error : "操作失败，请重试";
}

export function YouTubeUploadJobs({ model, onRevealPath, focusJobId, onNotice }: {
  model: YouTubeModel;
  onRevealPath: (path: string) => void;
  focusJobId?: string;
  onNotice?: (message: string) => void;
}) {
  const [subtitleJob, setSubtitleJob] = useState<YouTubeJob>();
  const [filter, setFilter] = useState("all");
  const [query, setQuery] = useState("");
  const [pending, setPending] = useState<Set<string>>(new Set());
  const pendingRef = useRef(new Set<string>());
  const bulkRef = useRef(false);
  const [bulkPending, setBulkPending] = useState(false);
  const [selectedIds, setSelectedIds] = useState(new Set<string>());
  const [actionError, setActionError] = useState("");
  useEffect(() => { if (focusJobId) { setFilter("all"); setQuery(""); } }, [focusJobId]);
  useEffect(() => {
    const currentIds = new Set(model.jobs.map((job) => job.id));
    setSelectedIds((ids) => [...ids].some((id) => !currentIds.has(id))
      ? new Set([...ids].filter((id) => currentIds.has(id))) : ids);
  }, [model.jobs]);
  const queued = model.jobs.filter((job) => job.status === "queued").length;
  const active = model.jobs.filter((job) => activeStatuses.includes(job.status)).length;
  const paused = model.jobs.filter((job) => job.status === "paused").length;
  const attention = model.jobs.filter((job) => attentionStatuses.includes(job.status)).length;
  const filters = [
    { id: "all", label: "全部", match: (_job: YouTubeJob) => true },
    { id: "active", label: "进行中", match: (job: YouTubeJob) => activeStatuses.includes(job.status) || job.status === "queued" },
    { id: "paused", label: "已暂停", match: (job: YouTubeJob) => job.status === "paused" },
    { id: "attention", label: "待处理", match: (job: YouTubeJob) => attentionStatuses.includes(job.status) },
    { id: "completed", label: "已完成", match: (job: YouTubeJob) => job.status === "completed" },
    { id: "cancelled", label: "已取消", match: (job: YouTubeJob) => job.status === "cancelled" },
  ];
  const visible = model.jobs.filter((job) => filters.find((item) => item.id === filter)!.match(job)
    && job.title.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()));
  const selectedVisible = visible.filter((job) => selectedIds.has(job.id));
  const allVisibleSelected = visible.length > 0 && selectedVisible.length === visible.length;
  const selectionBusy = bulkPending || selectedVisible.some((job) => pending.has(job.id));
  function selectRows(ids: string[], checked: boolean) {
    setSelectedIds((current) => {
      const next = new Set(current);
      for (const id of ids) { if (checked) next.add(id); else next.delete(id); }
      return next;
    });
  }
  async function deleteSelected() {
    if (bulkRef.current || !selectedVisible.length || selectedVisible.some((job) => pendingRef.current.has(job.id))) return;
    // Capture only selected visible records, even if the filter changes mid-request.
    const targets = selectedVisible;
    bulkRef.current = true;
    setBulkPending(true);
    for (const job of targets) pendingRef.current.add(job.id);
    setPending(new Set(pendingRef.current));
    setActionError("");
    try {
      const results = await Promise.allSettled(targets.map(async (job) => { await model.removeJob(job.id); }));
      const removed = targets.filter((_, index) => results[index].status === "fulfilled");
      selectRows(removed.map((job) => job.id), false);
      const failures = results.flatMap((result, index) => result.status === "rejected"
        ? [`${targets[index].title}：${actionErrorMessage(result.reason)}`] : []);
      if (failures.length) setActionError(`${failures.length} 项删除失败：${failures.join("；")}`);
      if (removed.length) onNotice?.(`已删除 ${removed.length} 项上传任务`);
    } finally {
      for (const job of targets) pendingRef.current.delete(job.id);
      setPending(new Set(pendingRef.current));
      bulkRef.current = false;
      setBulkPending(false);
    }
  }
  async function runAction(jobId: string, action: () => Promise<unknown>, deleting = false) {
    if (bulkRef.current || pendingRef.current.has(jobId)) return;
    pendingRef.current.add(jobId);
    setPending(new Set(pendingRef.current));
    setActionError("");
    try {
      await action();
      if (deleting) {
        selectRows([jobId], false);
        onNotice?.("已删除 1 项上传任务");
      }
    }
    catch (error) { setActionError(actionErrorMessage(error)); }
    finally { pendingRef.current.delete(jobId); setPending(new Set(pendingRef.current)); }
  }
  return (
    <div className="youtube-workspace">
      <div className="youtube-overview" aria-label="上传统计">
        <article><span>正在上传 / 处理</span><strong>{active}<small> / 5</small></strong></article>
        <article><span>等待上传</span><strong>{queued}</strong></article>
        <article><span>已暂停</span><strong>{paused}</strong></article>
        <article className={attention ? "needs-attention" : ""}><span>需要处理</span><strong>{attention}</strong></article>
      </div>
      <div className="youtube-queue-toolbar">
        <div className="upload-filters" role="group" aria-label="上传状态筛选">{filters.map((item) => <button type="button" key={item.id} aria-pressed={filter === item.id} className={filter === item.id ? "active" : ""} onClick={() => setFilter(item.id)}>{item.label}<span>{model.jobs.filter(item.match).length}</span></button>)}</div>
        <input type="search" aria-label="搜索上传任务" placeholder="搜索剧名" value={query} onChange={(event) => setQuery(event.target.value)} />
      </div>
      <div className="bulk-task-toolbar youtube-bulk-actions" role="group" aria-label="上传任务批量操作">
        <label><input className="task-select-all" type="checkbox" aria-label="全选当前可见上传任务" checked={allVisibleSelected}
          ref={(node) => { if (node) node.indeterminate = selectedVisible.length > 0 && !allVisibleSelected; }}
          disabled={!visible.length || bulkPending} onChange={(event) => selectRows(visible.map((job) => job.id), event.target.checked)} />全选当前可见</label>
        <span aria-live="polite">已选 {selectedVisible.length} 项（当前可见），共选 {selectedIds.size} 项</span>
        <button type="button" className="secondary-button danger" disabled={!selectedVisible.length || selectionBusy} onClick={() => void deleteSelected()}>批量删除</button>
      </div>
      <p className="queue-summary" role="status">最多同时上传 5 个 · 正在处理 {active} 个 · 排队 {queued} 个</p>
      {actionError ? <p className="warning-banner" role="alert">{actionError}</p> : null}
      <div className="media-job-list youtube-job-list">
      {!visible.length ? <div className="download-empty"><h3>{model.jobs.length ? "没有匹配的上传任务" : "尚无上传任务"}</h3><p>{model.jobs.length ? "试试其他状态或剧名" : "在下载任务详情中选择合并视频或整季去背景音乐视频，再上传到 YouTube"}</p>{model.jobs.length ? <button type="button" className="secondary-button" onClick={() => { setFilter("all"); setQuery(""); }}>显示全部任务</button> : null}</div> : null}
      {visible.map((job) => (
        <article className={`media-job-row youtube-job-card selectable-task-row status-${job.status}`} data-testid="youtube-job-row" data-focus-id={job.id} key={job.id}>
          <input className="task-row-checkbox youtube-job-checkbox" type="checkbox" aria-label={`选择上传任务：${job.title}`} checked={selectedIds.has(job.id)}
            disabled={bulkPending || pending.has(job.id)} onChange={(event) => selectRows([job.id], event.target.checked)} />
          <div className="media-job-copy">
            <div className="media-job-title"><strong>{job.title}</strong><span className="upload-status">{statusCopy(job)}</span></div>
            <p className="upload-file-meta"><span>{model.channels.find((channel) => channel.channelId === job.channelId)?.title || "YouTube 频道"}</span><span title={job.sourcePath}>{job.sourcePath.split(/[\\/]/).pop()}</span></p>
            <div className="upload-progress-label"><strong>{Math.round(job.percent)}<small>%</small></strong><span>{fileSize(job.uploadedBytes)} / {fileSize(job.totalBytes)}</span></div>
            <div className="progress-track" role="progressbar" aria-label={`${job.title}上传进度`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(job.percent)}><span style={{ width: `${job.percent}%` }} /></div>
            <small>字幕：{{ skipped: "未上传", pending: "待提交", submitted: "已提交 YouTube", failed: "上传失败" }[job.subtitleState || "skipped"]}</small>
            {job.subtitleError ? <small className="error-copy upload-error">{job.subtitleError}</small> : null}
            {job.errorMessage ? <small className="error-copy upload-error">{job.errorMessage}</small> : null}
            {job.youtubeUrl ? <YouTubeVideoLink key={job.youtubeUrl} url={job.youtubeUrl} /> : null}
          </div>
          <div className="media-job-actions upload-job-actions">
            {["queued", "preparingAuthorization", "creatingSession", "uploading", "waitingToRetry"].includes(job.status) ? <button type="button" className="secondary-button" disabled={bulkPending || pending.has(job.id)} onClick={() => void runAction(job.id, () => model.pause(job.id))}>暂停</button> : null}
            {job.status === "paused" ? <button type="button" className="primary-button compact" disabled={bulkPending || pending.has(job.id)} onClick={() => void runAction(job.id, () => model.resume(job.id))}>继续</button> : null}
            {job.status === "failed" || job.status === "cancelled" ? <button type="button" className="primary-button compact" disabled={bulkPending || pending.has(job.id)} onClick={() => void runAction(job.id, () => model.retry(job.id))}>重试</button> : null}
            {job.thumbnailState === "failed" && !activeStatuses.includes(job.status) ? <button type="button" className="primary-button compact" disabled={bulkPending || pending.has(job.id)} onClick={() => void runAction(job.id, () => model.retryThumbnail(job.id))}>仅重试封面</button> : null}
            {job.subtitleState === "failed" && !activeStatuses.includes(job.status) ? <button type="button" className="primary-button compact" disabled={bulkPending || pending.has(job.id)} onClick={() => void runAction(job.id, () => model.uploadSubtitle(job.id, null))}>仅重试字幕</button> : null}
            {job.videoId && !activeStatuses.includes(job.status) && job.status !== "queued" ? <button type="button" className="secondary-button" disabled={bulkPending || pending.has(job.id)} onClick={() => setSubtitleJob(job)}>上传字幕</button> : null}
            <button type="button" className="text-action" onClick={() => onRevealPath(job.sourcePath)}>源文件</button>
            {["queued", "pausing", "paused", "preparingAuthorization", "creatingSession", "uploading", "waitingToRetry", "processing"].includes(job.status) ? <button type="button" className="text-action upload-cancel" disabled={bulkPending || pending.has(job.id)} onClick={() => void runAction(job.id, () => model.cancel(job.id))}>取消</button> : null}
            <button type="button" className="text-action upload-delete" disabled={bulkPending || pending.has(job.id)} onClick={() => void runAction(job.id, () => model.removeJob(job.id), true)}>删除</button>
          </div>
        </article>
      ))}
      </div>
      {subtitleJob ? <SubtitleUploadDialog job={subtitleJob} onClose={() => setSubtitleJob(undefined)} onSubmit={(request) => model.uploadSubtitle(subtitleJob.id, request)} /> : null}
    </div>
  );
}
