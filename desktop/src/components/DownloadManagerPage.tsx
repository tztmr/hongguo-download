import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { deriveBatchStatus, type DownloadBatch, type DownloadItem } from "../download/model";
import type { DownloadManager } from "../download/useDownloadManager";
import { missingAiComponentIds } from "../media/aiRuntime";
import { completedMergeInputs, isCompletedBatch, pathIsWithin, sameFsPath, seriesRootFromInputs } from "../media/paths";
import type { MediaCommandError, MediaJob, MediaJobsModel, MergeSubmitOptions } from "../media/types";
import type { MediaJobScope } from "../media/types";
import type { AIComponentStatus, DemucsModel, WhisperModel } from "../types";
import type { NotificationTarget } from "../notifications";
import type { YouTubeModel, YouTubeUploadIntent } from "../youtube/types";
import { YouTubeUploadDialog } from "../youtube/YouTubeUploadDialog";
import { YouTubeUploadJobs } from "../youtube/YouTubeUploadJobs";
import { Cover } from "./Cover";
import { AlertIcon, CheckIcon, ChevronLeftIcon, CloseIcon, FolderIcon, PauseIcon, PlayIcon, RetryIcon, TrashIcon } from "./icons";
import { MergeVideoDialog } from "./MergeVideoDialog";
import { MediaScopeDialog } from "./MediaScopeDialog";
import { MediaJobsPanel } from "./MediaJobsPanel";
import { BatchMergeDialog, type BatchMergeTarget } from "./BatchMergeDialog";
import { YouTubeBatchUploadDialog, type YouTubeBatchUploadSource } from "../youtube/YouTubeBatchUploadDialog";

type ManagerSection = "downloads" | "media" | "youtube";

type DownloadManagerPageProps = {
  manager: DownloadManager;
  media: MediaJobsModel;
  saveDir: string;
  onOpenDir: () => void;
  onChooseDir: () => void;
  onRevealPath: (path: string) => void;
  demucsModel?: DemucsModel;
  whisperModel?: WhisperModel;
  aiComponents?: AIComponentStatus[];
  onInstallComponent?: (id: string) => Promise<void>;
  youtube?: YouTubeModel;
  focusTarget?: NotificationTarget | null;
  hidden?: boolean;
};

function formatBytes(value?: number) {
  if (!value) return "";
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${(value / 1024 / 1024).toFixed(1)} MB`;
}

function statusCopy(item: DownloadItem) {
  if (item.status === "running") return `下载中 ${Math.round(item.percent)}%`;
  if (item.status === "queued") return "等待中";
  if (item.status === "done") return "已完成";
  return "失败";
}

function batchCopy(batch: DownloadBatch) {
  if (batch.removeWhenIdle) return "完成当前集后移除";
  const status = deriveBatchStatus(batch);
  return { running: "下载中", queued: "等待中", paused: "已暂停", done: "已完成", error: "失败" }[status];
}

function batchProgress(batch: DownloadBatch) {
  if (!batch.items.length) return 0;
  return batch.items.reduce((sum, item) => sum + (item.status === "done" ? 100 : item.percent), 0) / batch.items.length;
}

function batchSeriesRoot(batch: DownloadBatch) {
  return isCompletedBatch(batch) ? seriesRootFromInputs(completedMergeInputs(batch)) : "";
}

function completedPathsFor(batch: DownloadBatch) {
  return batch.items.filter((item) => item.status === "done" && item.path).map((item) => item.path as string);
}

function includesPath(paths: Array<string | null | undefined>, candidate?: string | null) {
  return Boolean(candidate) && paths.some((path) => sameFsPath(path, candidate));
}

function matchesBatchSeries(batch: DownloadBatch, bookId?: string, seriesRoot?: string) {
  return Boolean(bookId && seriesRoot) && bookId === batch.bookId && sameFsPath(seriesRoot, batchSeriesRoot(batch));
}

function mergedPathFor(batch: DownloadBatch, jobs: MediaJob[]) {
  const completedPaths = completedPathsFor(batch);
  const seriesRoot = batchSeriesRoot(batch);
  return jobs.slice().reverse().find((job) =>
    job.kind === "merge" && job.status === "completed" && Boolean(job.outputPath)
      && (job.inputs.some((input) => includesPath(completedPaths, input.path))
        || matchesBatchSeries(batch, job.mergeRequest?.bookId, job.mergeRequest?.seriesRoot)
        || (Boolean(job.mergeRequest?.bookId) && job.mergeRequest?.bookId === batch.bookId && pathIsWithin(seriesRoot, job.outputPath))))?.outputPath || undefined;
}

function noBackgroundPathFor(batch: DownloadBatch, jobs: MediaJob[], mergedPath = mergedPathFor(batch, jobs)) {
  const seriesRoot = batchSeriesRoot(batch);
  return jobs.slice().reverse().find((job) =>
    job.kind === "separateBackgroundMusic" && job.status === "completed"
      && job.aiRequest?.scope === "merged"
      && (job.aiRequest.bookId && job.aiRequest.seriesRoot
        ? job.aiRequest.bookId === batch.bookId && sameFsPath(job.aiRequest.seriesRoot, seriesRoot)
        : Boolean(mergedPath) && job.inputs.some((input) => sameFsPath(input.path, mergedPath)))
      && job.outputs?.some((output) => output.kind === "noBackgroundMusicVideo"))
    ?.outputs?.find((output) => output.kind === "noBackgroundMusicVideo")?.path;
}

function batchForMediaJob(job: MediaJob, batches: DownloadBatch[], jobs: MediaJob[]) {
  if (job.aiRequest?.bookId && job.aiRequest.seriesRoot) {
    const exact = batches.find((batch) => matchesBatchSeries(batch, job.aiRequest?.bookId, job.aiRequest?.seriesRoot));
    if (exact) return exact;
  }
  if (job.mergeRequest?.bookId && job.mergeRequest.seriesRoot) {
    const exact = batches.find((batch) => matchesBatchSeries(batch, job.mergeRequest?.bookId, job.mergeRequest?.seriesRoot));
    if (exact) return exact;
  }
  const paths = job.inputs.map((input) => input.path);
  for (const merge of jobs) {
    if (merge.kind === "merge" && merge.outputPath && includesPath(paths, merge.outputPath)) {
      for (const input of merge.inputs) paths.push(input.path);
    }
  }
  return batches.find((batch) => batch.items.some((item) => item.path && includesPath(paths, item.path)));
}

export function DownloadManagerPage({
  manager,
  media,
  saveDir,
  onOpenDir,
  onChooseDir,
  onRevealPath,
  demucsModel = "htdemucs",
  whisperModel = "small",
  aiComponents,
  onInstallComponent,
  youtube,
  focusTarget,
  hidden = false,
}: DownloadManagerPageProps) {
  const [section, setSection] = useState<ManagerSection>("downloads");
  const [batchFilter, setBatchFilter] = useState("all");
  const [batchQuery, setBatchQuery] = useState("");
  const [checkedBatchIds, setCheckedBatchIds] = useState(new Set<string>());
  const [bulkMergeTargets, setBulkMergeTargets] = useState<BatchMergeTarget[] | null>(null);
  const [bulkUploadSources, setBulkUploadSources] = useState<YouTubeBatchUploadSource[] | null>(null);
  const [bulkPreparing, setBulkPreparing] = useState(false);
  const bulkPreparingRef = useRef(false);
  const [notice, setNotice] = useState<{ message: string } | null>(null);
  const showNotice = useCallback((message: string) => setNotice({ message }), []);
  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(null), 3000);
    return () => window.clearTimeout(timer);
  }, [notice]);
  const batchFilters = [
    { id: "all", label: "全部", match: (_batch: DownloadBatch) => true },
    { id: "active", label: "进行中", match: (batch: DownloadBatch) => ["running", "queued"].includes(deriveBatchStatus(batch)) },
    { id: "paused", label: "已暂停", match: (batch: DownloadBatch) => deriveBatchStatus(batch) === "paused" },
    { id: "done", label: "已完成", match: (batch: DownloadBatch) => deriveBatchStatus(batch) === "done" },
    { id: "error", label: "有失败", match: (batch: DownloadBatch) => batch.items.some((item) => item.status === "error") },
  ];
  const visibleBatches = manager.state.batches.filter((batch) => batchFilters.find((filter) => filter.id === batchFilter)!.match(batch)
    && batch.title.toLocaleLowerCase().includes(batchQuery.trim().toLocaleLowerCase()));
  const checkedBatches = visibleBatches.filter((batch) => checkedBatchIds.has(batch.id));
  const [selectedBatchId, setSelectedBatchId] = useState(manager.state.batches[0]?.id || "");
  const [detailOpen, setDetailOpen] = useState(false);
  const [mergingBatchId, setMergingBatchId] = useState<string | null>(null);
  const [mediaDialog, setMediaDialog] = useState<{ kind: "audioSeparation" | "subtitleExtraction"; batchId: string; mergedPath?: string } | null>(null);
  const [pendingInstall, setPendingInstall] = useState<{ kind: "audioSeparation" | "subtitleExtraction"; scope: MediaJobScope; ids: string[]; batchId: string; mergedPath?: string } | null>(null);
  const [installing, setInstalling] = useState(false);
  const [uploadDraft, setUploadDraft] = useState<{ batchId: string; sourcePath: string } | null>(null);
  const [mergedLookup, setMergedLookup] = useState<{ seriesRoot: string; exists: boolean; path?: string }>({ seriesRoot: "", exists: false });
  const [dismissedMediaError, setDismissedMediaError] = useState<MediaCommandError>();
  const [actionError, setActionError] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const submittingRef = useRef(false);
  const handledFocus = useRef<NotificationTarget | null>(null);
  const selectedBatch = manager.state.batches.find((batch) => batch.id === selectedBatchId) || manager.state.batches[0];
  const mergingBatch = manager.state.batches.find((batch) => batch.id === mergingBatchId) || null;
  const mediaDialogBatch = manager.state.batches.find((batch) => batch.id === mediaDialog?.batchId) || null;
  const uploadBatch = manager.state.batches.find((batch) => batch.id === uploadDraft?.batchId) || null;
  const completedPaths = selectedBatch ? completedPathsFor(selectedBatch) : [];
  const selectedSeriesRoot = selectedBatch ? batchSeriesRoot(selectedBatch) : "";
  const selectedMergedVideoPath = mergedLookup.seriesRoot === selectedSeriesRoot ? mergedLookup.path : undefined;
  const selectedHasMergedVideo = mergedLookup.seriesRoot === selectedSeriesRoot && mergedLookup.exists;
  const mergedPath = selectedBatch ? (mergedPathFor(selectedBatch, media.jobs) || selectedMergedVideoPath) : undefined;
  const noBackgroundPath = selectedBatch ? noBackgroundPathFor(selectedBatch, media.jobs, mergedPath) : undefined;
  const hasCompletedBackgroundSeparation = Boolean(selectedBatch && media.jobs.some((job) =>
    job.kind === "separateBackgroundMusic" && job.status === "completed"
      && (job.aiRequest?.bookId && job.aiRequest.seriesRoot
        ? job.aiRequest.bookId === selectedBatch.bookId && sameFsPath(job.aiRequest.seriesRoot, selectedSeriesRoot)
        : job.inputs.some((input) => includesPath(completedPaths, input.path) || sameFsPath(input.path, mergedPath)))
  ));
  const uploadSourcePath = noBackgroundPath || mergedPath;
  const uploadDisabledReason = !uploadSourcePath
    ? "请先完成合并视频或整季背景音乐分离"
    : !youtube?.credential.configured
      ? "请先在设置中导入 YouTube OAuth 凭证"
      : !youtube.activeChannelId ? "请先在设置中授权并选择 YouTube 频道" : undefined;
  useEffect(() => {
    let active = true;
    if (!selectedSeriesRoot) {
      setMergedLookup({ seriesRoot: selectedSeriesRoot, exists: false });
      return () => { active = false; };
    }
    if (media.findMergedVideo) {
      void media.findMergedVideo(selectedSeriesRoot).then(
        (path) => {
          if (!active) return;
          setMergedLookup({ seriesRoot: selectedSeriesRoot, path: path || undefined, exists: Boolean(path) });
        },
        () => {
          if (!active) return;
          setMergedLookup({ seriesRoot: selectedSeriesRoot, exists: false });
        },
      );
    } else {

      void media.hasMergedVideo(selectedSeriesRoot).then(
        (exists) => { if (active) setMergedLookup({ seriesRoot: selectedSeriesRoot, exists }); },
        () => { if (active) setMergedLookup({ seriesRoot: selectedSeriesRoot, exists: false }); },
      );
    }
    return () => { active = false; };
  }, [media.findMergedVideo, media.hasMergedVideo, media.jobs, selectedSeriesRoot]);
  useEffect(() => {
    if (!selectedBatch && selectedBatchId) setSelectedBatchId("");
    else if (selectedBatch && selectedBatch.id !== selectedBatchId) setSelectedBatchId(selectedBatch.id);
  }, [selectedBatch, selectedBatchId]);
  useEffect(() => {
    if (!focusTarget || handledFocus.current === focusTarget) return;
    handledFocus.current = focusTarget;
    if (focusTarget.kind === "downloadBatch") {
      setSection("downloads");
      setBatchFilter("all");
      setBatchQuery("");
      if (manager.state.batches.some((batch) => batch.id === focusTarget.id)) {
        setSelectedBatchId(focusTarget.id);
        setDetailOpen(true);
      }
    } else if (focusTarget.kind === "mediaJob") {
      setSection("media");
    } else if (focusTarget.kind === "youtubeJob") {
      setSection("youtube");
    }
    window.setTimeout(() => {
      document.querySelector<HTMLElement>(`[data-focus-id="${focusTarget.id}"]`)?.scrollIntoView?.({ block: "center" });
    }, 0);
  }, [focusTarget, manager.state.batches]);
  useEffect(() => {
    if (media.error?.code !== "MEDIA_JOB_ALREADY_ACTIVE" || media.error === dismissedMediaError) return;
    const timeout = window.setTimeout(() => setDismissedMediaError(media.error), 3_000);
    return () => window.clearTimeout(timeout);
  }, [dismissedMediaError, media.error]);
  const visibleMediaError = media.error && media.error !== dismissedMediaError ? media.error : undefined;
  const hasCompletedBatch = useMemo(
    () => manager.state.batches.some((batch) => batch.items.length > 0 && batch.items.every((item) => item.status === "done")),
    [manager.state.batches],
  );

  async function submitMerge(options: MergeSubmitOptions) {
    if (!mergingBatch) return;
    try {
      await media.startMerge(mergingBatch, options);
      setMergingBatchId(null);
      showNotice(`《${mergingBatch.title}》已加入合并队列`);
    } catch {
      // Stable-code errors are shown from media.error; keep the dialog open for correction.
    }
  }

  async function enqueueAI(batchId: string, kind: "audioSeparation" | "subtitleExtraction", scope: MediaJobScope, sourcePath?: string) {
    const batch = manager.state.batches.find((item) => item.id === batchId);
    if (!batch) return;
    const targetMergedPath = sourcePath || mergedPathFor(batch, media.jobs) || (selectedBatch?.id === batch.id ? selectedMergedVideoPath : undefined);
    if (kind === "audioSeparation") {
      await media.startAudioSeparation(batch, scope, demucsModel, targetMergedPath);
    } else {
      await media.startSubtitleExtraction(batch, scope, whisperModel, targetMergedPath);
    }
    setMediaDialog(null);
    setPendingInstall(null);
    showNotice(`《${batch.title}》已加入${kind === "audioSeparation" ? "背景音乐分离" : "字幕提取"}队列`);
  }

  async function submitAI(batchId: string, kind: "audioSeparation" | "subtitleExtraction", scope: MediaJobScope, sourcePath?: string) {
    if (submittingRef.current) return;
    const modelId = kind === "audioSeparation" ? `demucs-${demucsModel}` : `whisper-${whisperModel}`;
    const missing = missingAiComponentIds(aiComponents, modelId);
    if (missing.length) {
      setPendingInstall({ kind, scope, ids: missing, batchId, mergedPath: sourcePath });
      setMediaDialog(null);
      return;
    }
    submittingRef.current = true;
    setSubmitting(true);
    setActionError("");
    try { await enqueueAI(batchId, kind, scope, sourcePath); }
    catch (error) { setActionError(error && typeof error === "object" && "message" in error ? String(error.message) : "提交失败，请重试"); }
    finally { submittingRef.current = false; setSubmitting(false); }
  }

  async function installAndEnqueue() {
    if (!pendingInstall || !onInstallComponent) return;
    setInstalling(true);
    try {
      for (const id of pendingInstall.ids) await onInstallComponent(id);
      await enqueueAI(pendingInstall.batchId, pendingInstall.kind, pendingInstall.scope, pendingInstall.mergedPath);
    } catch (error) {
      setActionError(error && typeof error === "object" && "message" in error ? String(error.message) : "安装或提交失败，请重试");
    } finally {
      setInstalling(false);
    }
  }

  function toggleBatch(id: string, checked: boolean) {
    setCheckedBatchIds((current) => {
      const next = new Set(current);
      if (checked) next.add(id); else next.delete(id);
      return next;
    });
  }

  function removeCheckedBatches() {
    for (const batch of checkedBatches) manager.removeBatch(batch.id);
    const removed = new Set(checkedBatches.map((batch) => batch.id));
    setCheckedBatchIds((current) => new Set([...current].filter((id) => !removed.has(id))));
    showNotice(`已移除 ${removed.size} 个任务记录，正在下载的任务将在当前集结束后移除`);
  }

  async function prepareBulk(kind: "merge" | "upload") {
    if (bulkPreparingRef.current || !checkedBatches.length) return;
    bulkPreparingRef.current = true;
    setBulkPreparing(true);
    setActionError("");
    const mergeTargets: BatchMergeTarget[] = [];
    const uploadSources: YouTubeBatchUploadSource[] = [];
    const skipped: string[] = [];
    try {
      for (const batch of checkedBatches) {
        if (!isCompletedBatch(batch)) {
          mergeTargets.push({ batch, reason: "下载尚未完成" });
          skipped.push(`《${batch.title}》下载尚未完成`);
          continue;
        }
        try {
          const root = batchSeriesRoot(batch);
          const savedMerge = mergedPathFor(batch, media.jobs) || await media.findMergedVideo?.(root) || undefined;
          const exists = Boolean(savedMerge) || (kind === "merge" && await media.hasMergedVideo(root));
          mergeTargets.push({ batch, reason: exists ? "已有合并视频，已跳过" : undefined });
          if (kind === "upload") {
            const sourcePath = noBackgroundPathFor(batch, media.jobs, savedMerge) || savedMerge;
            if (sourcePath) uploadSources.push({ batch, sourcePath });
            else skipped.push(`《${batch.title}》尚无合并或分离成片`);
          }
        } catch {
          mergeTargets.push({ batch, reason: "无法检查成片，请重试" });
          skipped.push(`《${batch.title}》无法检查成片`);
        }
      }
      if (kind === "merge") setBulkMergeTargets(mergeTargets);
      else {
        if (uploadSources.length) setBulkUploadSources(uploadSources);
        if (skipped.length) setActionError(`已跳过 ${skipped.length} 部：${skipped.join("；")}`);
      }
    } finally { bulkPreparingRef.current = false; setBulkPreparing(false); }
  }

  return (
    <main className="download-page" hidden={hidden}>
      <header className="download-page-header">
        <div>
          <h1>下载管理</h1>
          <div className="save-path-row">
            <span>保存路径：</span><button type="button" className="path-button" onClick={onChooseDir}>{saveDir || "尚未选择"}</button>
          </div>
        </div>
        <div className="global-actions">
          <button type="button" className="secondary-button" onClick={onOpenDir}><FolderIcon />打开目录</button>
          {section === "downloads" ? <>
            <button type="button" className="secondary-button" onClick={manager.clearCompleted} disabled={!hasCompletedBatch}><TrashIcon />清理已完成</button>
            <button type="button" className="primary-button compact" onClick={manager.state.globallyPaused ? manager.resumeAll : manager.pauseAll}>
              {manager.state.globallyPaused ? <PlayIcon /> : <PauseIcon />}{manager.state.globallyPaused ? "全部继续" : "全部暂停"}
            </button>
          </> : null}
        </div>
      </header>

      <div className="manager-tabs" role="tablist" aria-label="下载管理分区">
        <button type="button" role="tab" aria-selected={section === "downloads"} className={section === "downloads" ? "active" : ""} onClick={() => setSection("downloads")}>下载任务 <span aria-hidden="true">{manager.state.batches.length}</span></button>
        <button type="button" role="tab" aria-selected={section === "media"} className={section === "media" ? "active" : ""} onClick={() => setSection("media")}>媒体处理 <span aria-hidden="true">{media.jobs.length}</span></button>
        <button type="button" role="tab" aria-selected={section === "youtube"} className={section === "youtube" ? "active" : ""} onClick={() => setSection("youtube")}>YouTube 上传 <span aria-hidden="true">{youtube?.jobs.length || 0}</span></button>
      </div>

      {manager.warning ? <div className="warning-banner"><AlertIcon />{manager.warning}</div> : null}
      {media.warning ? <div className="warning-banner"><AlertIcon />{media.warning}</div> : null}
      {visibleMediaError ? <div className="warning-banner" role="alert"><AlertIcon /><span>{visibleMediaError.message}</span><button type="button" className="icon-button warning-dismiss" aria-label="关闭提示" onClick={() => setDismissedMediaError(visibleMediaError)}><CloseIcon size={14} /></button></div> : null}
      {youtube?.error ? <div className="warning-banner" role="alert"><AlertIcon />{youtube.error.message}</div> : null}
      {actionError ? <div className="warning-banner" role="alert"><span>{actionError}</span><button type="button" className="icon-button" aria-label="关闭操作提示" onClick={() => setActionError("")}><CloseIcon size={14} /></button></div> : null}

      {section === "downloads" ? (
        <>
          <div className="download-control-row">
            <label>同时下载
              <select aria-label="同时下载" value={manager.state.concurrency} onChange={(event) => manager.setConcurrency(Number(event.target.value))}>
                {Array.from({ length: 10 }, (_, index) => index + 1).map((value) => <option value={value} key={value}>{value}</option>)}
              </select>
              <span>个任务（1–10）</span>
            </label>
          </div>

          <section className="stat-grid" aria-label="下载统计">
            <article><span>下载中</span><strong className="stat-running">{manager.stats.running}</strong></article>
            <article><span>等待中</span><strong>{manager.stats.queued}</strong></article>
            <article><span>已完成</span><strong className="stat-done">{manager.stats.done}</strong></article>
            <article><span>失败</span><strong className="stat-error">{manager.stats.error}</strong></article>
          </section>

          <section className={`download-workspace ${detailOpen ? "detail-open" : ""}`}>
            <div className="batch-panel">
              <div className="panel-title"><div><span className="title-marker" /><h2>批量任务</h2></div><span>显示 {visibleBatches.length} / {manager.state.batches.length} 个任务</span></div>
              {manager.state.batches.length ? <div className="batch-filter-toolbar">
                <input type="search" aria-label="搜索下载任务" placeholder="搜索已添加的剧名" value={batchQuery} onChange={(event) => setBatchQuery(event.target.value)} />
                <div className="upload-filters" role="group" aria-label="下载状态筛选">{batchFilters.map((filter) => <button type="button" key={filter.id} aria-pressed={batchFilter === filter.id} className={batchFilter === filter.id ? "active" : ""} onClick={() => setBatchFilter(filter.id)}>{filter.label}<span>{manager.state.batches.filter(filter.match).length}</span></button>)}</div>
              </div> : null}
              {manager.state.batches.length ? <div className="bulk-task-toolbar" role="group" aria-label="下载批量操作">
                <label><input type="checkbox" aria-label="全选当前下载任务" checked={visibleBatches.length > 0 && checkedBatches.length === visibleBatches.length}
                  ref={(input) => { if (input) input.indeterminate = checkedBatches.length > 0 && checkedBatches.length < visibleBatches.length; }}
                  onChange={(event) => { const checked = event.target.checked; setCheckedBatchIds((current) => { const next = new Set(current); for (const batch of visibleBatches) { if (checked) next.add(batch.id); else next.delete(batch.id); } return next; }); }} />全选当前列表</label>
                <span>已选 {checkedBatches.length} 项</span>
                <button type="button" className="secondary-button danger" disabled={!checkedBatches.length || bulkPreparing} onClick={removeCheckedBatches}>批量删除</button>
                <button type="button" className="secondary-button" disabled={!checkedBatches.length || bulkPreparing} onClick={() => void prepareBulk("merge")}>批量合并</button>
                <button type="button" className="secondary-button" disabled={!checkedBatches.length || bulkPreparing || !youtube?.credential.configured || !youtube.activeChannelId} title={!youtube?.activeChannelId ? "请先在设置中授权并选择 YouTube 频道" : undefined} onClick={() => void prepareBulk("upload")}>批量上传 YouTube</button>
                {bulkPreparing ? <span role="status">正在检查成片…</span> : null}
              </div> : null}
              {!manager.state.batches.length ? (
                <div className="download-empty"><div className="empty-download-icon"><FolderIcon size={26} /></div><h3>还没有下载任务</h3><p>从首页、搜索或榜单中选择剧集加入队列</p></div>
              ) : !visibleBatches.length ? (
                <div className="download-empty"><h3>没有匹配的下载任务</h3><p>试试其他状态或剧名</p><button type="button" className="secondary-button" onClick={() => { setBatchFilter("all"); setBatchQuery(""); }}>显示全部下载任务</button></div>
              ) : (
                <div className="batch-list">
                  {visibleBatches.map((batch) => {
                    const completed = batch.items.filter((item) => item.status === "done").length;
                    const failed = batch.items.filter((item) => item.status === "error").length;
                    const progress = batchProgress(batch);
                    return (
                      <article className={`batch-row ${selectedBatch?.id === batch.id ? "selected" : ""}`} data-testid="download-batch-row" data-focus-id={batch.id} key={batch.id}>
                        <input className="task-row-checkbox" type="checkbox" aria-label={`选择下载任务 ${batch.title}`} checked={checkedBatchIds.has(batch.id)} onChange={(event) => toggleBatch(batch.id, event.target.checked)} />
                        <button type="button" className="batch-main" aria-label={`查看 ${batch.title} 任务详情`} onClick={() => { setSelectedBatchId(batch.id); setDetailOpen(true); }}>
                          <Cover src={batch.cover} title={batch.title} className="batch-cover" />
                          <div className="batch-copy">
                            <div className="batch-title-line"><h3>{batch.title}</h3><span>{completed} / {batch.items.length} 集</span></div>
                            <div className="progress-track"><span style={{ width: `${progress}%` }} /></div>
                            <div className="batch-meta"><span className={`status-${deriveBatchStatus(batch)}`}>{batchCopy(batch)}</span>{failed ? <span>{failed} 项失败</span> : null}<span>{Math.round(progress)}%</span></div>
                          </div>
                        </button>
                        <div className="batch-row-actions">
                          <button type="button" className="icon-button" title={batch.paused ? "继续任务" : "暂停任务"} onClick={() => batch.paused ? manager.resumeBatch(batch.id) : manager.pauseBatch(batch.id)}>{batch.paused ? <PlayIcon /> : <PauseIcon />}</button>
                          <button type="button" className="icon-button danger" title="移除任务记录" onClick={() => manager.removeBatch(batch.id)}><TrashIcon /></button>
                        </div>
                      </article>
                    );
                  })}
                </div>
              )}
            </div>

            <aside className="download-detail" aria-label="任务详情">
              {selectedBatch ? (
                <>
                  <div className="detail-mobile-head"><button type="button" className="icon-button" aria-label="返回任务列表" onClick={() => setDetailOpen(false)}><ChevronLeftIcon /></button><span>任务详情</span></div>
                  <div className="panel-title"><div><span className="title-marker" /><h2>任务详情</h2></div><span>{selectedBatch.items.length} 集</span></div>
                  <header className="detail-summary">
                    <Cover src={selectedBatch.cover} title={selectedBatch.title} className="detail-cover" />
                    <div><h3>{selectedBatch.title}</h3><p>{selectedBatch.items.filter((item) => item.status === "done").length} / {selectedBatch.items.length} 集 · {batchCopy(selectedBatch)}</p></div>
                  </header>
                  <div className="detail-actions media-ready-actions">
                    <button type="button" className="secondary-button" onClick={() => selectedBatch.paused ? manager.resumeBatch(selectedBatch.id) : manager.pauseBatch(selectedBatch.id)}>
                      {selectedBatch.paused ? <PlayIcon /> : <PauseIcon />}{selectedBatch.paused ? "继续此任务" : "暂停此任务"}
                    </button>
                    <button type="button" className="secondary-button" onClick={() => manager.retryBatch(selectedBatch.id)} disabled={!selectedBatch.items.some((item) => item.status === "error")}><RetryIcon />重试失败项</button>
                    <button
                      type="button"
                      className="primary-button"
                      title={selectedHasMergedVideo ? "已存在合并视频，请先移走或删除后再合并" : undefined}
                      onClick={() => { if (!selectedHasMergedVideo) setMergingBatchId(selectedBatch.id); }}
                      disabled={!isCompletedBatch(selectedBatch) || selectedHasMergedVideo}
                    >{selectedHasMergedVideo ? "已合并" : "合并视频"}</button>
                    <button
                      type="button"
                      className="secondary-button"
                      disabled={!isCompletedBatch(selectedBatch) || hasCompletedBackgroundSeparation}
                      title={hasCompletedBackgroundSeparation ? "该剧已完成背景音乐分离" : undefined}
                      onClick={() => { if (!hasCompletedBackgroundSeparation) setMediaDialog({ kind: "audioSeparation", batchId: selectedBatch.id, mergedPath }); }}
                    >{hasCompletedBackgroundSeparation ? "背景音乐已分离" : "分离背景音乐"}</button>
                    <button type="button" className="secondary-button" disabled={!isCompletedBatch(selectedBatch)} onClick={() => setMediaDialog({ kind: "subtitleExtraction", batchId: selectedBatch.id, mergedPath })}>提取字幕</button>
                    <button
                      type="button"
                      className="secondary-button"
                      disabled={Boolean(uploadDisabledReason)}
                      title={uploadDisabledReason}
                      onClick={() => { if (uploadSourcePath) setUploadDraft({ batchId: selectedBatch.id, sourcePath: uploadSourcePath }); }}
                    >上传 YouTube</button>
                  </div>
                  <div className="episode-task-list">
                    {selectedBatch.items.map((item) => (
                      <article className={`episode-task status-${item.status}`} key={item.id}>
                        <div className="episode-state-icon">{item.status === "done" ? <CheckIcon /> : item.status === "error" ? <AlertIcon /> : <span>{item.episodeIndex}</span>}</div>
                        <div className="episode-task-copy">
                          <div><strong>{item.episodeTitle}</strong><span>{statusCopy(item)}</span></div>
                          <div className="progress-track"><span style={{ width: `${item.status === "done" ? 100 : item.percent}%` }} /></div>
                          {item.status === "done" ? <small>已完成文件 · {formatBytes(item.total)}</small> : null}
                          {item.error ? <small className="error-copy" title={item.error}>{item.error}</small> : null}
                        </div>
                        <div className="episode-actions">
                          {item.status === "error" ? <button type="button" className="text-action accent" onClick={() => manager.retryItem(item.id)}>重试</button> : null}
                          {item.status === "queued" || item.status === "error" ? <button type="button" className="text-action" onClick={() => manager.removeItem(item.id)}>移除</button> : null}
                          {item.status === "done" && item.path ? <button type="button" className="text-action" onClick={() => onRevealPath(item.path!)}>定位</button> : null}
                        </div>
                      </article>
                    ))}
                  </div>
                </>
              ) : (
                <div className="download-empty compact-empty"><h3>选择一个任务</h3><p>逐集状态和操作会显示在这里</p></div>
              )}
            </aside>
          </section>
        </>
      ) : null}

      <div hidden={section !== "media"} className="manager-section-content">
        <MediaJobsPanel
          media={media}
          batches={manager.state.batches}
          onRevealPath={onRevealPath}
          onShowDownloads={() => setSection("downloads")}
          focusId={focusTarget?.kind === "mediaJob" ? focusTarget.id : undefined}
          youtubeUploadDisabledReason={(job) => {
            if (!batchForMediaJob(job, manager.state.batches, media.jobs)) return "对应的下载任务已被移除";
            if (!youtube?.credential.configured) return "请先在设置中导入 YouTube OAuth 凭证";
            if (!youtube.activeChannelId) return "请先在设置中授权并选择 YouTube 频道";
            return undefined;
          }}
          onUploadToYouTube={(job, sourcePath) => {
            const batch = batchForMediaJob(job, manager.state.batches, media.jobs);
            if (batch) setUploadDraft({ batchId: batch.id, sourcePath });
          }}
          onNotice={showNotice}
          separationDisabledReason={(job) => {
            const batch = batchForMediaJob(job, manager.state.batches, media.jobs);
            if (!batch) return "对应的下载任务已被移除";
            if (!isCompletedBatch(batch)) return "请先完成该剧下载";
            const exists = media.jobs.some((candidate) => candidate.kind === "separateBackgroundMusic"
              && ["queued", "running", "paused", "completed"].includes(candidate.status)
              && candidate.inputs.some((input) => sameFsPath(input.path, job.outputPath)));
            return exists ? "该视频已分离或正在分离队列中" : undefined;
          }}
          onSeparateVideo={(job, sourcePath) => {
            const batch = batchForMediaJob(job, manager.state.batches, media.jobs);
            if (batch) void submitAI(batch.id, "audioSeparation", "merged", sourcePath);
          }}
          onBulkUploadToYouTube={(sources) => {
            const drafts = sources.flatMap(({ job, sourcePath }) => {
              const batch = batchForMediaJob(job, manager.state.batches, media.jobs);
              return batch ? [{ batch, sourcePath }] : [];
            });
            if (drafts.length) setBulkUploadSources(drafts);
          }}
        />
      </div>

      <div hidden={section !== "youtube"} className="manager-section-content">
        <section className="media-job-panel" aria-label="YouTube 上传">
          {youtube ? <YouTubeUploadJobs model={youtube} onRevealPath={onRevealPath} onNotice={showNotice} focusJobId={focusTarget?.kind === "youtubeJob" ? focusTarget.id : undefined} /> : <div className="download-empty"><h3>尚无上传任务</h3></div>}
        </section>
      </div>

      {mergingBatch ? (
        <MergeVideoDialog
          batch={mergingBatch}
          onSubmit={(options) => {
            void submitMerge(options);
          }}
          onClose={() => setMergingBatchId(null)}
        />
      ) : null}
      {mediaDialogBatch && mediaDialog ? (
        <MediaScopeDialog
          kind={mediaDialog.kind}
          busy={submitting}
          hasMergedVideo={Boolean(mediaDialog.mergedPath || mergedPathFor(mediaDialogBatch, media.jobs) || (selectedBatch?.id === mediaDialogBatch.id ? selectedMergedVideoPath : undefined))}
          title={mediaDialogBatch.title}
          episodeCount={mediaDialogBatch.items.length}
          modelName={mediaDialog.kind === "audioSeparation" ? demucsModel : `Whisper ${whisperModel}`}
          onSubmit={(scope) => { void submitAI(mediaDialog.batchId, mediaDialog.kind, scope, mediaDialog.mergedPath); }}
          onClose={() => setMediaDialog(null)}
        />
      ) : null}
      {pendingInstall ? (
        <div className="dialog-backdrop" role="presentation">
          <section className="merge-dialog" role="dialog" aria-modal="true" aria-label="安装 AI 组件">
            <header><div><span className="title-marker" /><h2>安装 AI 组件</h2></div></header>
            <p>本次需要先下载并安装：</p>
            <ul>{pendingInstall.ids.map((id) => {
              const component = aiComponents?.find((item) => item.id === id);
              return <li key={id}>{id}{component ? ` · 下载 ${formatBytes(component.downloadBytes)} · 安装 ${formatBytes(component.installedBytes)}` : " · 未在发布清单中配置"}</li>;
            })}</ul>
            <footer><button type="button" className="secondary-button" disabled={installing} onClick={() => setPendingInstall(null)}>取消</button><button type="button" className="primary-button" disabled={installing || !onInstallComponent || pendingInstall.ids.some((id) => !aiComponents?.some((item) => item.id === id))} onClick={() => { void installAndEnqueue(); }}>{installing ? "安装中…" : "确认安装并继续"}</button></footer>
          </section>
        </div>
      ) : null}
      {uploadBatch && uploadDraft && youtube ? (
        <YouTubeUploadDialog
          batch={uploadBatch}
          sourcePath={uploadDraft.sourcePath}
          channelId={youtube.activeChannelId || ""}
          onClose={() => setUploadDraft(null)}
          onSubmit={async (request: YouTubeUploadIntent) => {
            await youtube.startUpload(request);
            setUploadDraft(null);
            showNotice(`《${uploadBatch.title}》已加入 YouTube 上传队列`);
          }}
        />
      ) : null}
      {bulkMergeTargets ? <BatchMergeDialog targets={bulkMergeTargets} onSubmit={media.startMerge} onClose={() => setBulkMergeTargets(null)} onQueued={(count) => showNotice(`已加入 ${count} 个合并任务`)} /> : null}
      {bulkUploadSources && youtube ? <YouTubeBatchUploadDialog sources={bulkUploadSources} channelId={youtube.activeChannelId || ""} onSubmit={youtube.startUpload} onClose={() => setBulkUploadSources(null)} onQueued={(count) => showNotice(`已加入 ${count} 个 YouTube 上传任务`)} /> : null}
      {notice ? <div className="toast manager-toast" role="status"><span><CheckIcon size={14} /></span>{notice.message}<button type="button" aria-label="关闭消息" onClick={() => setNotice(null)}><CloseIcon size={16} /></button></div> : null}
    </main>
  );
}
