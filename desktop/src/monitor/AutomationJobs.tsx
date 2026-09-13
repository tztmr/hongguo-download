import { useEffect, useState } from "react";
import type { AutomationJob } from "./automationRuntime";

const labels: Record<string, string> = { inspect: "检查与查重", download: "下载剧集", merge: "智能合并 / H.264 转码", separate: "分离背景音乐", subtitles: "提取字幕", metadata: "AI 文案与封面", upload: "上传与远端处理", short: "首集 Shorts 处理", cleanup: "校验与清理", done: "全部完成" };
const statuses = { pending: "等待 / 后台处理中", working: "处理中", review: "待核对", failed: "处理失败", completed: "完成", skipped: "已跳过", observing: "稍后自动重试" };
type Filter = "active" | "attention" | "completed" | "all";
const finished = (job: AutomationJob) => job.status === "completed" || job.status === "skipped";
const attention = (job: AutomationJob) => job.status === "failed" || job.status === "review";
const time = (value: number) => new Date(value * 1000).toLocaleString("zh-CN", { hour12: false });

export function AutomationJobs({ jobs, loaded, pending, onAction }: { jobs: AutomationJob[]; loaded: boolean; pending: boolean; onAction: (id: string, action: "continue" | "skip" | "retry") => void }) {
  const [filter, setFilter] = useState<Filter>("active");
  const [query, setQuery] = useState("");
  const [page, setPage] = useState(1);
  const counts = { active: jobs.filter(job => !finished(job)).length, attention: jobs.filter(attention).length, completed: jobs.filter(finished).length, all: jobs.length };
  const visible = jobs.filter(job => (filter === "all" || filter === "active" && !finished(job) || filter === "attention" && attention(job) || filter === "completed" && finished(job)) && `${job.title} ${job.bookId}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()))
    .sort((a, b) => Number(b.status === "working") - Number(a.status === "working") || Number(attention(b)) - Number(attention(a)) || b.updatedAt - a.updatedAt);
  const pages = Math.max(1, Math.ceil(visible.length / 20));
  const currentPage = Math.min(page, pages);
  useEffect(() => setPage(1), [filter, query]);
  return <div className="auto-task-board">
    <div className="auto-task-toolbar"><div className="auto-task-filters" aria-label="后台任务筛选">{([['active', '未完成'], ['attention', '需关注'], ['completed', '已结束'], ['all', '全部']] as const).map(([id, label]) => <button type="button" key={id} aria-pressed={filter === id} onClick={() => setFilter(id)}>{label}<span>{counts[id]}</span></button>)}</div><input type="search" aria-label="搜索后台任务" placeholder="搜索剧名或 ID" value={query} onChange={e => setQuery(e.target.value)} /></div>
    <div className="auto-runtime-jobs">{!visible.length ? <p className="auto-runtime-empty">{!loaded ? "正在读取后台状态…" : !jobs.length ? "尚无后台任务。保存并启动后显示真实进度。" : filter === "active" && !query ? "当前任务已全部结束，可在「已结束」查看上传与清理结果。" : "没有符合条件的任务"}</p> : visible.slice((currentPage - 1) * 20, currentPage * 20).map(job => {
      const progress = Math.round(Math.max(0, Math.min(100, job.progress || 0)));
      return <article className={`auto-runtime-job status-${job.status}`} key={job.id} aria-label={`${job.title}任务`}>
        <header className="auto-task-title"><strong>{job.title}{job.season && <small>第 {job.season} 季</small>}</strong><span className="auto-task-status">{statuses[job.status]}</span></header>
        <div className="auto-task-stage"><b>{labels[job.stage] || job.stage}</b>{!finished(job) && <span>阶段进度 {progress}%</span>}</div>
        {!finished(job) && <progress max={100} value={progress} aria-label={`${job.title}进度`} />}
        <p className="auto-task-message">{job.message}</p>
        {job.episodeTotal > 0 && <p className="auto-runtime-meta">{job.episodeDone >= job.episodeTotal ? "下载已完成" : "已下载"} {job.episodeDone}/{job.episodeTotal} 集</p>}
        {!!job.retryAt && job.retryAt > Date.now() / 1000 && (job.status === "pending" || job.status === "observing") && <p className="auto-task-retry">{job.attempts ? `第 ${job.attempts} 次重试` : "自动复查"} · 预计 {time(job.retryAt)} · 等待自动恢复，复用已完成文件</p>}
        <footer className="auto-task-footer"><details><summary>任务详情</summary><p>ID：{job.bookId} · 更新：{time(job.updatedAt)}</p><p>{job.message}</p></details><div className="auto-runtime-actions">
          {job.status === "review" && <><button type="button" disabled={pending} onClick={() => onAction(job.id, "continue")}>确认继续处理</button><button type="button" disabled={pending} onClick={() => onAction(job.id, "skip")}>跳过此任务</button></>}
          {job.status === "failed" && <button type="button" disabled={pending} onClick={() => onAction(job.id, "retry")}>重试此任务</button>}
          {job.mainVideoUrl && <a href={job.mainVideoUrl} target="_blank" rel="noreferrer">查看正片 ↗</a>}{job.shortVideoUrl && <a href={job.shortVideoUrl} target="_blank" rel="noreferrer">查看首集 Shorts ↗</a>}
        </div></footer>
      </article>;
    })}</div>
    {pages > 1 && <div className="auto-task-pages"><span>第 {currentPage} / {pages} 页 · {visible.length} 个任务</span><button type="button" disabled={currentPage === 1} onClick={() => setPage(currentPage - 1)}>上一页</button><button type="button" disabled={currentPage === pages} onClick={() => setPage(currentPage + 1)}>下一页</button></div>}
  </div>;
}
