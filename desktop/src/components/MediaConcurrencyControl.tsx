import { useState } from "react";

export function MediaConcurrencyControl({ value = 0, onChange, disabled = false, max = 10 }: {
  value?: number; onChange: (value: number) => Promise<void>; disabled?: boolean; max?: number;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return <div className="media-concurrency-control">
    <label>AI 同时处理
      <select aria-label="AI 同时处理" value={value} disabled={disabled || busy} onChange={async event => {
        const next = Number(event.target.value);
        setBusy(true); setError("");
        try { await onChange(next); }
        catch { setError("并发设置保存失败，请重试"); }
        finally { setBusy(false); }
      }}>
        <option value={0}>自动（最多 {max} 个）</option>
        {Array.from({ length: max }, (_, i) => i + 1).map(n => <option value={n} key={n}>{n} 个任务</option>)}
      </select>
    </label>
    <small>仅分离背景音乐与提取字幕共用名额；下载、合并和 YouTube 上传走独立队列。完成一个自动补入一个；降低上限不打断正在处理的任务，内存或显存不足时等待。</small>
    {error ? <p role="alert">{error}</p> : null}
  </div>;
}
