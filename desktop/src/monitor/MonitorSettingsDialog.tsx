import { useEffect, useRef, useState } from "react";

export function MonitorSettingsDialog({ days, onSave, onClose }: {
  days: number | null; onSave: (days: number | null) => void; onClose: () => void;
}) {
  const [value, setValue] = useState(String(days ?? 7));
  const [error, setError] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const valid = /^\d+$/.test(value) && Number(value) >= 1 && Number(value) <= 30;
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    input.current?.focus();
    return () => previous?.focus();
  }, []);
  function save(next: number | null) {
    try { onSave(next); onClose(); } catch (cause) { setError(cause instanceof Error ? cause.message : "设置保存失败，请重试"); }
  }
  return <div className="dialog-backdrop" onMouseDown={e => { if (e.target === e.currentTarget) onClose(); }}>
    <section className="merge-dialog monitor-settings-dialog" role="dialog" aria-modal="true" aria-label="新剧监听设置" onKeyDown={event => {
      if (event.key === "Escape") onClose();
      if (event.key !== "Tab") return;
      const controls = [...event.currentTarget.querySelectorAll<HTMLElement>('input, button:not(:disabled)')];
      const first = controls[0], last = controls[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }}>
      <h2>新剧监听设置</h2><p>按剧目的实际上线日期筛选，保存后立即重新检查。</p>
      <form onSubmit={event => { event.preventDefault(); if (valid) save(Number(value)); }}>
        <label className="monitor-days-input">最近天数<input ref={input} type="number" min="1" max="30" step="1" value={value} onChange={e => setValue(e.target.value)} /></label>
        <div className="monitor-day-presets">{[1, 3, 7, 15, 30].map(day => <button type="button" key={day} aria-pressed={Number(value) === day} onClick={() => setValue(String(day))}>{day === 1 ? "今天" : `最近 ${day} 天`}</button>)}</div>
        <p>包含今天，按北京时间自然日计算。例如最近 3 天为今天、昨天和前天。未知上线时间的剧目不计入天数范围。</p>
        <p>设置适用于真人剧、漫剧和 AI 剧。漫剧和 AI 剧受新剧榜可提供的历史范围限制。</p>
        {!valid && <p role="alert">请输入 1–30 之间的整数天数</p>}{error && <p role="alert">{error}</p>}
        <footer><button type="button" onClick={() => save(null)}>恢复默认范围</button><button type="button" onClick={onClose}>取消</button><button type="submit" className="primary-button" disabled={!valid}>保存并刷新</button></footer>
      </form>
    </section>
  </div>;
}
