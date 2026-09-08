import { useEffect, useState } from "react";
import type { DownloadBatch } from "../download/model";
import type { MediaJob, MediaJobsModel } from "../media/types";
import { AlertIcon, CheckIcon, FolderIcon, PauseIcon, PlayIcon, QueueIcon, SearchIcon, TrashIcon } from "./icons";

const kinds = { merge: "合并视频", separateBackgroundMusic: "分离背景音乐", extractSubtitles: "提取字幕" };
const statuses = { queued: "排队中", running: "处理中", paused: "已暂停", completed: "已完成", failed: "失败", cancelled: "已取消", interrupted: "已中断" };
const stages: Record<string, string> = {
  queued: "等待处理", running: "正在处理", paused: "已暂停", probing: "检查视频", merging: "合并视频", separating: "分离音轨",
  "stream-copy": "无损合并", "audio-normalizing": "保留画面 · 校正音轨", normalizing: "统一视频规格", "hardware-transcode": "硬件转码",
  "software-fallback": "软件转码", validating: "校验成片时间轴",
  transcribing: "识别字幕", exportingSubtitle: "导出字幕", completed: "处理完成", failed: "处理失败", cancelled: "已取消", interrupted: "任务中断",
};
const outputKinds = { vocals: "人声", backgroundMusic: "背景音乐", noBackgroundMusicVideo: "去背景音乐视频", subtitles: "SRT 字幕" };
type Filter = "all" | "active" | "completed" | "attention";
function matches(job: MediaJob, filter: Filter) {
  return filter === "all" || (filter === "active" ? ["running", "queued", "paused"].includes(job.status)
    : filter === "completed" ? job.status === "completed" : ["failed", "cancelled", "interrupted"].includes(job.status));
}
function sourceTitle(job: MediaJob, batches: DownloadBatch[], jobs: MediaJob[]) {
  const savedTitle = job.mergeRequest?.title || job.aiRequest?.title;
  if (savedTitle) return savedTitle;
  const paths = new Set(job.inputs.map((input) => input.path));
  // AI tasks may use the output of a merge instead of the original episode paths.
  for (const merge of jobs) {
    if (merge.kind === "merge" && merge.outputPath && paths.has(merge.outputPath)) {
      for (const input of merge.inputs) paths.add(input.path);
    }
  }
  return batches.find((batch) => batch.items.some((item) => item.path && paths.has(item.path)))?.title
    || job.outputPath?.split(/[\\/]/).pop() || job.inputs[0]?.path.split(/[\\/]/).pop() || "媒体任务";
}

export function MediaJobsPanel({ media, batches, onRevealPath, onShowDownloads, focusId, onUploadToYouTube, youtubeUploadDisabledReason }: {
  media: MediaJobsModel;
  batches: DownloadBatch[];
  onRevealPath: (path: string) => void;
  onShowDownloads: () => void;
  focusId?: string;
  onUploadToYouTube?: (job: MediaJob, sourcePath: string) => void;
  youtubeUploadDisabledReason?: (job: MediaJob) => string | undefined;
}) {
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [busyIds, setBusyIds] = useState<string[]>([]);
  const [actionError, setActionError] = useState("");
  useEffect(() => { if (focusId) { setFilter("all"); setQuery(""); } }, [focusId]);
  const filters: Array<{ id: Filter; label: string }> = [
    { id: "all", label: "全部任务" }, { id: "active", label: "进行中" }, { id: "completed", label: "已完成" }, { id: "attention", label: "需处理" },
  ];
  const visible = media.jobs.filter((job) => matches(job, filter) && `${sourceTitle(job, batches, media.jobs)} ${kinds[job.kind]}`.toLowerCase().includes(query.trim().toLowerCase()));
  async function act(job: MediaJob, action: "pause" | "resume" | "deleteJob" | "retry") {
    setBusyIds((ids) => [...ids, job.id]);
    setActionError("");
    try { await media[action](job.id); }
    catch (error) { setActionError(error instanceof Error ? error.message : "操作失败，请重试"); }
    finally { setBusyIds((ids) => ids.filter((id) => id !== job.id)); }
  }
  return (
    <section className="media-workspace" aria-label="媒体处理">
      <div className="media-overview">
        <div><h2>媒体处理</h2><p>背景音乐分离按 CPU / GPU 资源自动调度，最多同时处理 5 个任务。</p></div>
        <button type="button" className="secondary-button" onClick={onShowDownloads}><QueueIcon />从下载任务创建</button>
      </div>
      <div className="media-toolbar">
        <div className="media-filters" role="group" aria-label="媒体任务状态">
          {filters.map((item) => <button type="button" key={item.id} aria-pressed={filter === item.id} onClick={() => setFilter(item.id)}>{item.label}<span>{media.jobs.filter((job) => matches(job, item.id)).length}</span></button>)}
        </div>
        <label className="media-search"><SearchIcon size={16} /><input type="search" aria-label="搜索媒体任务" placeholder="搜索剧名或处理类型" value={query} onChange={(event) => setQuery(event.target.value)} /></label>
      </div>
      {actionError ? <p className="warning-banner" role="alert">{actionError}</p> : null}
      <div className="media-job-list media-task-cards">
        {!visible.length ? <div className="download-empty"><div className="empty-download-icon"><QueueIcon size={26} /></div><h3>{media.jobs.length ? "没有匹配的媒体任务" : "还没有媒体任务"}</h3><p>{media.jobs.length ? "试试其他状态或搜索关键词" : "先完成剧集下载，再选择合并视频、分离背景音乐或提取字幕。"}</p>{media.jobs.length ? <button type="button" className="secondary-button" onClick={() => { setFilter("all"); setQuery(""); }}>清除筛选</button> : <button type="button" className="primary-button" onClick={onShowDownloads}>查看下载任务</button>}</div> : visible.map((job) => {
          const percent = job.status === "completed" ? 100 : Math.round(Number.isFinite(job.percent) ? Math.max(0, Math.min(100, job.percent)) : 0);
          const stage = job.kind === "separateBackgroundMusic" && job.status === "queued"
            ? "等待队列与可用计算资源"
            : job.stage.replace(/\b[a-zA-Z]+\b/g, (word) => stages[word] || word);
          const uploadSource = job.kind === "separateBackgroundMusic" && job.status === "completed" && job.aiRequest?.scope === "merged"
            ? job.outputs?.find((output) => output.kind === "noBackgroundMusicVideo")?.path
            : undefined;
          const uploadDisabledReason = uploadSource ? youtubeUploadDisabledReason?.(job) : undefined;
          return (
            <article className={`media-job-row media-task-card status-${job.status}`} data-testid="media-job-row" data-focus-id={job.id} key={job.id}>
              <div className={`media-task-icon status-${job.status}`}>{job.status === "completed" ? <CheckIcon size={21} /> : ["failed", "interrupted"].includes(job.status) ? <AlertIcon size={21} /> : <QueueIcon size={21} />}</div>
              <div className="media-job-copy">
                <div className="media-job-title"><strong title={sourceTitle(job, batches, media.jobs)}>{sourceTitle(job, batches, media.jobs)}</strong><span className={`media-status status-${job.status}`}>{statuses[job.status]}</span></div>
                <div className="media-task-meta"><span>{kinds[job.kind]}</span><span>{job.inputs.length} 个输入文件</span>{job.aiRequest ? <span>{job.aiRequest.scope === "merged" ? "合并视频" : "逐集处理"} · {job.aiRequest.model}</span> : null}</div>
                <div className="media-progress-label"><span>{stage || statuses[job.status]}</span><strong>{percent}%</strong></div>
                <div className="progress-track" role="progressbar" aria-label={`${kinds[job.kind]}进度`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}><span style={{ width: `${percent}%` }} /></div>
                {job.errorMessage ? <p className="media-task-error">{job.errorMessage}</p> : null}
                {job.outputPath ? <small className="media-result-path" title={job.outputPath}>输出 · {job.outputPath}</small> : null}
                {job.outputs?.length ? <details className="media-outputs"><summary>输出文件 <span>{job.outputs.length}</span></summary><div>{job.outputs.map((output) => <div className="media-output-file" key={`${output.kind}-${output.episodeIndex}-${output.path}`}><span>{output.episodeIndex > 0 ? `第 ${output.episodeIndex} 集 · ` : ""}{outputKinds[output.kind]}<small title={output.path}>{output.path}</small></span><button type="button" className="text-action" onClick={() => onRevealPath(output.path)}>定位</button></div>)}</div></details> : null}
              </div>
              <div className="media-job-actions">
                {["queued", "running"].includes(job.status) ? <button type="button" className="secondary-button" disabled={busyIds.includes(job.id)} onClick={() => void act(job, "pause")}><PauseIcon size={15} />暂停</button> : null}
                {job.status === "paused" ? <button type="button" className="secondary-button" disabled={busyIds.includes(job.id)} onClick={() => void act(job, "resume")}><PlayIcon size={15} />继续</button> : null}
                {["failed", "cancelled", "interrupted"].includes(job.status) ? <button type="button" className="secondary-button" disabled={busyIds.includes(job.id)} onClick={() => void act(job, "retry")}>重试</button> : null}
                {job.status === "completed" && job.outputPath ? <button type="button" className="secondary-button" onClick={() => onRevealPath(job.outputPath!)}><FolderIcon size={15} />定位</button> : null}
                {uploadSource ? <button type="button" className="primary-button compact" disabled={Boolean(uploadDisabledReason)} title={uploadDisabledReason} onClick={() => onUploadToYouTube?.(job, uploadSource)}>上传 YouTube</button> : null}
                <button type="button" className="secondary-button danger" disabled={busyIds.includes(job.id)} onClick={() => void act(job, "deleteJob")}><TrashIcon size={15} />删除</button>
              </div>
            </article>
          );
        })}
      </div>
    </section>
  );
}
