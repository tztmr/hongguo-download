import type { YouTubeJob, YouTubeModel } from "./types";

function statusCopy(job: YouTubeJob) {
  return {
    queued: "排队中",
    preparingAuthorization: "准备授权",
    creatingSession: "创建上传会话",
    uploading: "上传中",
    waitingToRetry: "等待重试",
    processing: "YouTube 处理中",
    settingThumbnail: "设置封面",
    completed: "已完成",
    videoUploadedThumbnailFailed: "视频已上传，封面失败",
    failed: "失败",
    cancelled: "已取消",
  }[job.status];
}

export function YouTubeUploadJobs({ model, onRevealPath }: { model: YouTubeModel; onRevealPath: (path: string) => void }) {
  if (!model.jobs.length) return <div className="download-empty"><h3>尚无上传任务</h3><p>完成合并后，可从任务详情上传到 YouTube</p></div>;
  return (
    <div className="media-job-list">
      {model.jobs.map((job) => (
        <article className={`media-job-row status-${job.status}`} data-testid="youtube-job-row" data-focus-id={job.id} key={job.id}>
          <div className="media-job-copy">
            <div className="media-job-title"><strong>{job.title}</strong><span>{statusCopy(job)}</span></div>
            <div className="progress-track" role="progressbar" aria-label={`${job.title}上传进度`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(job.percent)}><span style={{ width: `${job.percent}%` }} /></div>
            <small>{Math.round(job.percent)}% · {(job.uploadedBytes / 1024 / 1024).toFixed(1)} / {(job.totalBytes / 1024 / 1024).toFixed(1)} MB</small>
            {job.youtubeUrl ? <a href={job.youtubeUrl} target="_blank" rel="noreferrer">打开 YouTube 视频</a> : null}
            {job.errorMessage ? <small className="error-copy">{job.errorMessage}</small> : null}
          </div>
          <div className="media-job-actions">
            <button type="button" className="text-action" onClick={() => onRevealPath(job.sourcePath)}>源文件</button>
            {["queued", "preparingAuthorization", "creatingSession", "uploading", "waitingToRetry", "processing"].includes(job.status) ? <button type="button" className="text-action" onClick={() => void model.cancel(job.id)}>取消</button> : null}
            {job.status === "failed" || job.status === "cancelled" ? <button type="button" className="text-action accent" onClick={() => void model.retry(job.id)}>重试</button> : null}
            {job.status === "videoUploadedThumbnailFailed" ? <button type="button" className="text-action accent" onClick={() => void model.retryThumbnail(job.id)}>仅重试封面</button> : null}
          </div>
        </article>
      ))}
    </div>
  );
}
