import { useState } from "react";
import { SubtitlePicker, subtitleRequest, type SubtitleChoice } from "./SubtitlePicker";
import { useUploadPreferences } from "./uploadPreferences";
import type { YouTubeJob, YouTubeUploadIntent } from "./types";

export function SubtitleUploadDialog({ job, onClose, onSubmit }: { job: YouTubeJob; onClose: () => void; onSubmit: (request: YouTubeUploadIntent["subtitle"]) => Promise<void> }) {
  const { settings, setSetting } = useUploadPreferences();
  const [choice, setChoice] = useState<SubtitleChoice>();
  const [available, setAvailable] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function submit() {
    if (busy) return;
    setBusy(true); setError("");
    try { await onSubmit(subtitleRequest(choice, job.sourcePath, settings.subtitleLanguage)); onClose(); }
    catch (reason) { setError(reason && typeof reason === "object" && "message" in reason ? String(reason.message) : "字幕提交失败，请重试"); }
    finally { setBusy(false); }
  }
  return <div className="dialog-backdrop" role="presentation"><section className="merge-dialog youtube-upload-dialog" role="dialog" aria-modal="true" aria-label="上传字幕">
    <header><h2>上传字幕</h2><button type="button" aria-label="关闭" className="icon-button" disabled={busy} onClick={onClose}>×</button></header>
    <p>{job.title} · 字幕将添加到已上传的视频</p>
    <SubtitlePicker sourcePath={job.sourcePath} value={choice} onChange={setChoice} language={settings.subtitleLanguage} onLanguageChange={(value) => setSetting("subtitleLanguage", value)} disabled={busy} existingVideo onAvailabilityChange={setAvailable} />
    {error ? <p role="alert" className="warning-banner">{error}</p> : null}
    <footer><button className="secondary-button" disabled={busy} onClick={onClose}>取消</button><button className="primary-button" disabled={busy || !available} onClick={() => void submit()}>{busy ? "正在上传字幕…" : "确认上传字幕"}</button></footer>
  </section></div>;
}
