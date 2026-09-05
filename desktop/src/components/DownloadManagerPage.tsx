import { useEffect, useMemo, useState } from "react";
import { deriveBatchStatus, type DownloadBatch, type DownloadItem } from "../download/model";
import type { DownloadManager } from "../download/useDownloadManager";
import { isCompletedBatch } from "../media/paths";
import type { MediaJob, MediaJobsModel, MergeSubmitOptions } from "../media/types";
import type { MediaJobScope } from "../media/types";
import type { AIComponentStatus, DemucsModel, WhisperModel } from "../types";
import type { NotificationTarget } from "../notifications";
import type { YouTubeModel, YouTubeUploadIntent } from "../youtube/types";
import { YouTubeUploadDialog } from "../youtube/YouTubeUploadDialog";
import { YouTubeUploadJobs } from "../youtube/YouTubeUploadJobs";
import { Cover } from "./Cover";
import { AlertIcon, CheckIcon, ChevronLeftIcon, FolderIcon, PauseIcon, PlayIcon, RetryIcon, TrashIcon } from "./icons";
import { MergeVideoDialog } from "./MergeVideoDialog";
import { MediaScopeDialog } from "./MediaScopeDialog";

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

function mediaStatusCopy(job: MediaJob) {
  return {
    queued: "排队中",
    running: "处理中",
    completed: "已完成",
    failed: "失败",
    cancelled: "已取消",
    interrupted: "已中断",
  }[job.status];
}

function mediaKindCopy(job: MediaJob) {
  return {
    merge: "合并视频",
    separateBackgroundMusic: "分离背景音乐",
    extractSubtitles: "提取字幕",
  }[job.kind];
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
}: DownloadManagerPageProps) {
  const [section, setSection] = useState<ManagerSection>("downloads");
  const [selectedBatchId, setSelectedBatchId] = useState(manager.state.batches[0]?.id || "");
  const [detailOpen, setDetailOpen] = useState(false);
  const [mergingBatchId, setMergingBatchId] = useState<string | null>(null);
  const [mediaDialogKind, setMediaDialogKind] = useState<"audioSeparation" | "subtitleExtraction" | null>(null);
  const [pendingInstall, setPendingInstall] = useState<{ kind: "audioSeparation" | "subtitleExtraction"; scope: MediaJobScope; ids: string[] } | null>(null);
  const [installing, setInstalling] = useState(false);
  const [uploadingBatchId, setUploadingBatchId] = useState<string | null>(null);
  const selectedBatch = manager.state.batches.find((batch) => batch.id === selectedBatchId) || manager.state.batches[0];
  const mergingBatch = manager.state.batches.find((batch) => batch.id === mergingBatchId) || null;
  const completedPaths = new Set((selectedBatch?.items || []).filter((item) => item.status === "done" && item.path).map((item) => item.path as string));
  const mergedPath = media.jobs.slice().reverse().find((job) =>
    job.kind === "merge" && job.status === "completed" && Boolean(job.outputPath)
      && job.inputs.some((input) => completedPaths.has(input.path)))?.outputPath || undefined;
  const noBackgroundPath = media.jobs.slice().reverse().find((job) =>
    job.kind === "separateBackgroundMusic" && job.status === "completed"
      && job.inputs.some((input) => completedPaths.has(input.path) || input.path === mergedPath)
      && job.outputs?.some((output) => output.kind === "noBackgroundMusicVideo"))
    ?.outputs?.find((output) => output.kind === "noBackgroundMusicVideo")?.path;
  const uploadSourcePath = noBackgroundPath || mergedPath;
  useEffect(() => {
    if (!selectedBatch && selectedBatchId) setSelectedBatchId("");
    else if (selectedBatch && selectedBatch.id !== selectedBatchId) setSelectedBatchId(selectedBatch.id);
  }, [selectedBatch, selectedBatchId]);
  useEffect(() => {
    if (!focusTarget) return;
    if (focusTarget.kind === "downloadBatch") {
      setSection("downloads");
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
  const hasCompletedBatch = useMemo(
    () => manager.state.batches.some((batch) => batch.items.length > 0 && batch.items.every((item) => item.status === "done")),
    [manager.state.batches],
  );

  async function submitMerge(options: MergeSubmitOptions) {
    if (!mergingBatch) return;
    try {
      await media.startMerge(mergingBatch, options);
      setMergingBatchId(null);
      setSection("media");
    } catch {
      // Stable-code errors are shown from media.error; keep the dialog open for correction.
    }
  }

  async function enqueueAI(kind: "audioSeparation" | "subtitleExtraction", scope: MediaJobScope) {
    if (!selectedBatch) return;
    if (kind === "audioSeparation") {
      await media.startAudioSeparation(selectedBatch, scope, demucsModel, mergedPath);
    } else {
      await media.startSubtitleExtraction(selectedBatch, scope, whisperModel, mergedPath);
    }
    setMediaDialogKind(null);
    setPendingInstall(null);
    setSection("media");
  }

  async function submitAI(kind: "audioSeparation" | "subtitleExtraction", scope: MediaJobScope) {
    const modelId = kind === "audioSeparation" ? `demucs-${demucsModel}` : `whisper-${whisperModel}`;
    const ids = ["runtime", modelId];
    const missing = aiComponents === undefined
      ? []
      : ids.filter((id) => !aiComponents.some((component) => component.id === id && component.installed));
    if (missing.length) {
      setPendingInstall({ kind, scope, ids: missing });
      setMediaDialogKind(null);
      return;
    }
    await enqueueAI(kind, scope);
  }

  async function installAndEnqueue() {
    if (!pendingInstall || !onInstallComponent) return;
    setInstalling(true);
    try {
      for (const id of pendingInstall.ids) await onInstallComponent(id);
      await enqueueAI(pendingInstall.kind, pendingInstall.scope);
    } finally {
      setInstalling(false);
    }
  }

  return (
    <main className="download-page">
      <header className="download-page-header">
        <div>
          <h1>下载管理</h1>
          <div className="save-path-row">
            <span>保存路径：</span><button type="button" className="path-button" onClick={onChooseDir}>{saveDir || "尚未选择"}</button>
          </div>
        </div>
        <div className="global-actions">
          <button type="button" className="secondary-button" onClick={onOpenDir}><FolderIcon />打开目录</button>
          <button type="button" className="secondary-button" onClick={manager.clearCompleted} disabled={!hasCompletedBatch}><TrashIcon />清理已完成</button>
          <button type="button" className="primary-button compact" onClick={manager.state.globallyPaused ? manager.resumeAll : manager.pauseAll}>
            {manager.state.globallyPaused ? <PlayIcon /> : <PauseIcon />}{manager.state.globallyPaused ? "全部继续" : "全部暂停"}
          </button>
        </div>
      </header>

      <div className="manager-tabs" role="tablist" aria-label="下载管理分区">
        <button type="button" role="tab" aria-selected={section === "downloads"} className={section === "downloads" ? "active" : ""} onClick={() => setSection("downloads")}>下载任务 <span aria-hidden="true">{manager.state.batches.length}</span></button>
        <button type="button" role="tab" aria-selected={section === "media"} className={section === "media" ? "active" : ""} onClick={() => setSection("media")}>媒体处理 <span aria-hidden="true">{media.jobs.length}</span></button>
        <button type="button" role="tab" aria-selected={section === "youtube"} className={section === "youtube" ? "active" : ""} onClick={() => setSection("youtube")}>YouTube 上传 <span aria-hidden="true">{youtube?.jobs.length || 0}</span></button>
      </div>

      {manager.warning ? <div className="warning-banner"><AlertIcon />{manager.warning}</div> : null}
      {media.warning ? <div className="warning-banner"><AlertIcon />{media.warning}</div> : null}
      {media.error ? <div className="warning-banner" role="alert"><AlertIcon />{media.error.message}</div> : null}
      {youtube?.error ? <div className="warning-banner" role="alert"><AlertIcon />{youtube.error.message}</div> : null}

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
              <div className="panel-title"><div><span className="title-marker" /><h2>批量任务</h2></div><span>共 {manager.state.batches.length} 个任务</span></div>
              {!manager.state.batches.length ? (
                <div className="download-empty"><div className="empty-download-icon"><FolderIcon size={26} /></div><h3>还没有下载任务</h3><p>从首页、搜索或榜单中选择剧集加入队列</p></div>
              ) : (
                <div className="batch-list">
                  {manager.state.batches.map((batch) => {
                    const completed = batch.items.filter((item) => item.status === "done").length;
                    const failed = batch.items.filter((item) => item.status === "error").length;
                    const progress = batchProgress(batch);
                    return (
                      <article className={`batch-row ${selectedBatch?.id === batch.id ? "selected" : ""}`} data-testid="download-batch-row" data-focus-id={batch.id} key={batch.id}>
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
                    <button type="button" className="primary-button" onClick={() => setMergingBatchId(selectedBatch.id)} disabled={!isCompletedBatch(selectedBatch)}>合并视频</button>
                    <button type="button" className="secondary-button" disabled={!isCompletedBatch(selectedBatch)} onClick={() => setMediaDialogKind("audioSeparation")}>分离背景音乐</button>
                    <button type="button" className="secondary-button" disabled={!isCompletedBatch(selectedBatch)} onClick={() => setMediaDialogKind("subtitleExtraction")}>提取字幕</button>
                    <button
                      type="button"
                      className="secondary-button"
                      disabled={!uploadSourcePath || !youtube?.credential.configured || !youtube.activeChannelId}
                      title={!uploadSourcePath ? "请先完成合并视频" : !youtube?.activeChannelId ? "请先在设置中授权并选择 YouTube 频道" : undefined}
                      onClick={() => setUploadingBatchId(selectedBatch.id)}
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

      {section === "media" ? (
        <section className="media-job-panel" aria-label="媒体处理">
          {!media.jobs.length ? (
            <div className="download-empty"><h3>还没有媒体任务</h3><p>完成下载后，可以在任务详情中合并视频</p></div>
          ) : (
            <div className="media-job-list">
              {media.jobs.map((job) => (
                <article className={`media-job-row status-${job.status}`} data-testid="media-job-row" data-focus-id={job.id} key={job.id}>
                  <div className="media-job-copy">
                    <div className="media-job-title">
                      <strong>{mediaKindCopy(job)}</strong>
                      <span className={`status-${job.status}`}>{mediaStatusCopy(job)}</span>
                    </div>
                    <div className="progress-track" role="progressbar" aria-label={`${mediaKindCopy(job)}进度`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(job.status === "completed" ? 100 : job.percent)}><span style={{ width: `${job.status === "completed" ? 100 : job.percent}%` }} /></div>
                    <small>{job.stage} · {Math.round(job.percent)}%</small>
                    {job.outputPath ? <small>输出 {job.outputPath}</small> : null}
                    {job.outputs?.map((output) => (
                      <small className="media-output" key={`${output.kind}-${output.episodeIndex}-${output.path}`}>
                        第 {output.episodeIndex} 集 · {output.kind === "vocals" ? "人声" : output.kind === "backgroundMusic" ? "背景音乐" : output.kind === "noBackgroundMusicVideo" ? "去背景音乐视频" : "SRT 字幕"}
                        <button type="button" className="text-action" onClick={() => onRevealPath(output.path)}>定位</button>
                      </small>
                    ))}
                    {job.kind === "separateBackgroundMusic" ? <small className="copyright-note">分离可能残留或失真，不保证规避 Content ID 或版权责任。</small> : null}
                    {job.errorMessage ? <small className="error-copy">{job.errorMessage}</small> : null}
                  </div>
                  <div className="media-job-actions">
                    {job.status === "running" ? <button type="button" className="text-action" onClick={() => void media.cancel(job.id)}>取消</button> : null}
                    {job.status === "failed" || job.status === "cancelled" || job.status === "interrupted" ? (
                      <button type="button" className="text-action accent" onClick={() => void media.retry(job.id)}>重试</button>
                    ) : null}
                    {job.status === "completed" && job.outputPath ? (
                      <button type="button" className="text-action" onClick={() => onRevealPath(job.outputPath!)}>定位</button>
                    ) : null}
                  </div>
                </article>
              ))}
            </div>
          )}
        </section>
      ) : null}

      {section === "youtube" ? (
        <section className="media-job-panel" aria-label="YouTube 上传">
          {youtube ? <YouTubeUploadJobs model={youtube} onRevealPath={onRevealPath} /> : <div className="download-empty"><h3>尚无上传任务</h3></div>}
        </section>
      ) : null}

      {mergingBatch ? (
        <MergeVideoDialog
          batch={mergingBatch}
          onSubmit={(options) => {
            void submitMerge(options);
          }}
          onClose={() => setMergingBatchId(null)}
        />
      ) : null}
      {selectedBatch && mediaDialogKind ? (
        <MediaScopeDialog
          kind={mediaDialogKind}
          hasMergedVideo={Boolean(mergedPath)}
          onSubmit={(scope) => { void submitAI(mediaDialogKind, scope); }}
          onClose={() => setMediaDialogKind(null)}
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
      {selectedBatch && uploadingBatchId === selectedBatch.id && uploadSourcePath && youtube ? (
        <YouTubeUploadDialog
          batch={selectedBatch}
          sourcePath={uploadSourcePath}
          onClose={() => setUploadingBatchId(null)}
          onSubmit={(request: YouTubeUploadIntent) => {
            void youtube.startUpload(request).then(() => {
              setUploadingBatchId(null);
              setSection("youtube");
            });
          }}
        />
      ) : null}
    </main>
  );
}
