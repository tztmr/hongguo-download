import { useRef, useState } from "react";
import type { DownloadBatch } from "../download/model";
import { safeOutputFileName } from "../media/paths";
import type { MergeMode, MergeQuality, MergeSubmitOptions } from "../media/types";

export type BatchMergeTarget = { batch: DownloadBatch; reason?: string };

export function BatchMergeDialog({ targets, onSubmit, onClose, onQueued }: {
  targets: BatchMergeTarget[];
  onSubmit: (batch: DownloadBatch, options: MergeSubmitOptions) => Promise<unknown>;
  onClose: () => void;
  onQueued: (count: number) => void;
}) {
  const [mode, setMode] = useState<MergeMode>("auto");
  const [quality, setQuality] = useState<MergeQuality>("high");
  const [queued, setQueued] = useState(new Set<string>());
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const remaining = targets.filter(({ batch, reason }) => !reason && !queued.has(batch.id));

  async function submit() {
    if (inFlight.current || !remaining.length) return;
    inFlight.current = true;
    setBusy(true);
    let count = 0;
    let failed = false;
    for (const { batch } of remaining) {
      try {
        await onSubmit(batch, { outputName: safeOutputFileName(batch.title), mode, quality, conflictPolicy: "failIfExists" });
        setQueued((ids) => new Set(ids).add(batch.id));
        setErrors((current) => { const next = { ...current }; delete next[batch.id]; return next; });
        count += 1;
      } catch (reason) {
        failed = true;
        setErrors((current) => ({ ...current, [batch.id]: reason && typeof reason === "object" && "message" in reason ? String(reason.message) : "提交失败，请重试" }));
      }
    }
    setBusy(false);
    inFlight.current = false;
    if (count) onQueued(count);
    if (!failed) onClose();
  }

  return <div className="dialog-backdrop" role="presentation">
    <section className="merge-dialog batch-operation-dialog" role="dialog" aria-modal="true" aria-label="批量合并视频">
      <header><h2>批量合并视频</h2></header>
      <p>每部剧分别生成一个成片，按集数顺序合并。</p>
      <ul className="bulk-review-list">{targets.map(({ batch, reason }) => <li key={batch.id}>
        <strong>{batch.title}</strong><small>{safeOutputFileName(batch.title)} · {batch.items.length} 集</small>
        <span className={errors[batch.id] ? "error-copy" : ""}>{reason || (queued.has(batch.id) ? "已加入合并队列" : errors[batch.id] || "待合并")}</span>
      </li>)}</ul>
      <label>合并方式<select aria-label="批量合并方式" disabled={busy} value={mode} onChange={(e) => setMode(e.target.value as MergeMode)}>
        <option value="auto">智能合并（推荐）</option><option value="copy">仅无损合并</option><option value="transcode">统一转为 H.264</option>
      </select></label>
      <label>转码画质<select aria-label="批量转码画质" disabled={busy || mode === "copy"} value={quality} onChange={(e) => setQuality(e.target.value as MergeQuality)}>
        <option value="high">画质优先</option><option value="balanced">均衡</option><option value="compact">小体积</option>
      </select></label>
      <footer><button className="secondary-button" disabled={busy} onClick={onClose}>取消</button>
        <button className="primary-button" disabled={busy || !remaining.length} onClick={() => void submit()}>{busy ? "正在加入队列…" : Object.keys(errors).length ? "重试失败项" : "开始批量合并"}</button></footer>
    </section>
  </div>;
}
