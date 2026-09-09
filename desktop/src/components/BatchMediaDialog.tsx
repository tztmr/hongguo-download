import { useRef, useState, type ReactNode } from "react";
import type { DownloadBatch } from "../download/model";
import type { MediaJobScope } from "../media/types";

export type BatchMediaTarget = { batch: DownloadBatch; mergedPath?: string; reason?: string };
export type BatchMediaKind = "audioSeparation" | "subtitleExtraction";

export function BatchMediaDialog({ targets, kind, modelName, missingComponents, canInstall, onInstall, onSubmit, onClose, onQueued, concurrencyControl }: {
  concurrencyControl?: ReactNode;
  targets: BatchMediaTarget[];
  kind: BatchMediaKind;
  modelName: string;
  missingComponents: string[];
  canInstall: boolean;
  onInstall: () => Promise<void>;
  onSubmit: (target: BatchMediaTarget, scope: MediaJobScope) => Promise<unknown>;
  onClose: () => void;
  onQueued: (count: number) => void;
}) {
  const [scope, setScope] = useState<MediaJobScope>("episodes");
  const [queued, setQueued] = useState(new Set<string>());
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [installError, setInstallError] = useState("");
  const inFlight = useRef(false);
  const reasonFor = (target: BatchMediaTarget) => target.reason || (scope === "merged" && !target.mergedPath ? "尚无已验证的合并视频，已跳过" : undefined);
  const remaining = targets.filter(target => !reasonFor(target) && !queued.has(target.batch.id));
  const label = kind === "audioSeparation" ? "批量分离背景音乐" : "批量提取字幕";

  async function submit() {
    if (inFlight.current || !remaining.length) return;
    inFlight.current = true;
    setBusy(true);
    setInstallError("");
    let count = 0;
    let failed = false;
    try {
      if (missingComponents.length) await onInstall();
      for (const target of remaining) {
        try {
          await onSubmit(target, scope);
          setQueued(ids => new Set(ids).add(target.batch.id));
          setErrors(current => { const next = { ...current }; delete next[target.batch.id]; return next; });
          count += 1;
        } catch (error) {
          failed = true;
          setErrors(current => ({ ...current, [target.batch.id]: error && typeof error === "object" && "message" in error ? String(error.message) : "提交失败，请重试" }));
        }
      }
      if (count) onQueued(count);
      if (!failed) onClose();
    } catch (error) {
      setInstallError(error && typeof error === "object" && "message" in error ? String(error.message) : "组件安装失败，请重试");
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  }

  return <div className="dialog-backdrop" role="presentation">
    <section className="merge-dialog batch-operation-dialog" role="dialog" aria-modal="true" aria-label={label}>
      <header><h2>{label}</h2></header>
      <p>每部剧分别加入处理队列。当前模型：{modelName}（可在设置中修改）。</p>
      <fieldset disabled={busy} className="media-concurrency-field">{concurrencyControl}</fieldset>
      <div className="scope-options" role="radiogroup" aria-label="批量处理范围">
        <label><input type="radio" name="bulk-media-scope" disabled={busy || queued.size > 0} checked={scope === "episodes"} onChange={() => setScope("episodes")} />逐集处理</label>
        <label><input type="radio" name="bulk-media-scope" disabled={busy || queued.size > 0} checked={scope === "merged"} onChange={() => setScope("merged")} />合并视频</label>
      </div>
      <ul className="bulk-review-list">{targets.map(target => <li key={target.batch.id}>
        <strong>{target.batch.title}</strong><small>{target.batch.items.length} 集</small>
        <span className={errors[target.batch.id] ? "error-copy" : ""}>{queued.has(target.batch.id) ? "已加入处理队列" : reasonFor(target) || errors[target.batch.id] || "待处理"}</span>
      </li>)}</ul>
      {missingComponents.length ? <p>需要先安装 AI 组件：{missingComponents.join("、")}{!canInstall ? "。请先在设置中安装所需组件。" : "。确认后安装并继续。"}</p> : null}
      {installError ? <p role="alert" className="error-copy">{installError}</p> : null}
      <footer><button className="secondary-button" disabled={busy} onClick={onClose}>取消</button>
        <button className="primary-button" disabled={busy || !remaining.length || (missingComponents.length > 0 && !canInstall)} onClick={() => void submit()}>{busy ? "正在准备任务…" : missingComponents.length ? "确认安装并批量处理" : Object.keys(errors).length ? "重试失败项" : "开始批量处理"}</button></footer>
    </section>
  </div>;
}
