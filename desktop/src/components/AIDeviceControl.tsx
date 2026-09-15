import { useRef, useState } from "react";
import { errorMessage } from "../errors";
import type { AIDevicePreference } from "../types";

export function AIDeviceControl({ value = "auto", onChange }: {
  value?: AIDevicePreference; onChange(value: AIDevicePreference): Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const pending = useRef(false);
  return <div className="media-concurrency-control">
    <label>AI 计算设备<select aria-label="AI 计算设备" value={value} disabled={busy} onChange={async event => {
      if (pending.current) return;
      const next = event.target.value as AIDevicePreference;
      pending.current = true; setBusy(true); setError("");
      try { await onChange(next); } catch (reason) { setError(`设备设置保存失败：${errorMessage(reason)}，请重试`); }
      finally { pending.current = false; setBusy(false); }
    }}><option value="auto">{/Windows/i.test(navigator.userAgent) ? "自动分配 GPU + CPU" : "自动选择计算设备"}</option><option value="cpu">仅使用 CPU</option><option value="cuda">NVIDIA GPU</option></select></label>
    <small>Windows 自动模式会在资源足够时，把排队中的分离和字幕任务分配给 GPU、CPU 并行处理；仅同时处理 1 个时依次执行。此设置立即保存，新建 AI 任务生效，正在运行的任务保持原设备。</small>
    {busy ? <p role="status">正在保存计算设备…</p> : null}
    {error ? <p role="alert">{error}</p> : null}
  </div>;
}
