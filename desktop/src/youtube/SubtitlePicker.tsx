import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { findYouTubeSubtitle } from "./subtitleCommands";

export type SubtitleChoice = { sourcePath: string; path: string | null; disabled?: boolean };
export function subtitleRequest(choice: SubtitleChoice | undefined, sourcePath: string, language: string) {
  return choice?.sourcePath === sourcePath && choice.disabled ? null
    : { path: choice?.sourcePath === sourcePath ? choice.path : null, language };
}
export function SubtitlePicker({ sourcePath, value, onChange, language, onLanguageChange, disabled = false, existingVideo = false, onAvailabilityChange }: {
  sourcePath: string; value?: SubtitleChoice; onChange: (value: SubtitleChoice) => void;
  language: string; onLanguageChange: (value: string) => void; disabled?: boolean; existingVideo?: boolean; onAvailabilityChange?: (available: boolean) => void;
}) {
  const [match, setMatch] = useState<{ source: string; path: string | null; error?: string }>();
  const [error, setError] = useState("");
  const selected = value?.sourcePath === sourcePath ? value : undefined;
  const current = match?.source === sourcePath ? match : undefined;
  useEffect(() => {
    let active = true;
    if (sourcePath) void findYouTubeSubtitle(sourcePath).then((path) => {
      if (active) setMatch({ source: sourcePath, path });
    }).catch(() => { if (active) setMatch({ source: sourcePath, path: null, error: "暂时无法查找字幕，提交时会重新检查，也可手动选择" }); });
    return () => { active = false; };
  }, [sourcePath]);
  const available = Boolean(sourcePath && !selected?.disabled && (selected?.path || current?.path));
  useEffect(() => { onAvailabilityChange?.(available); }, [available, onAvailabilityChange]);
  async function choose() {
    setError("");
    try {
      const path = await open({ multiple: false, directory: false, filters: [{ name: "SRT 字幕", extensions: ["srt"] }] });
      if (typeof path === "string") onChange({ sourcePath, path });
    } catch { setError("无法打开字幕选择窗口，请重试"); }
  }
  return <fieldset className="youtube-subtitle-picker" disabled={disabled || !sourcePath}>
    <legend>{existingVideo ? "字幕文件" : "字幕（可选）"}</legend>
    <label>字幕语言<select aria-label="字幕语言" value={language} onChange={(event) => onLanguageChange(event.target.value)}>
      <option value="zh-Hans">中文（简体）</option><option value="zh-Hant">中文（繁体）</option>
      <option value="en">英语</option><option value="ja">日语</option><option value="ko">韩语</option><option value="es">西班牙语</option>
      <option value="pt">葡萄牙语</option><option value="vi">越南语</option><option value="th">泰语</option><option value="id">印尼语</option>
    </select></label>
    <div className="cover-picker">
      <button type="button" className="secondary-button" onClick={() => void choose()}>选择 SRT 字幕</button>
      <button type="button" className="text-action" onClick={() => onChange({ sourcePath, path: null })}>自动匹配</button>
      {!existingVideo ? <button type="button" className="text-action" onClick={() => onChange({ sourcePath, path: null, disabled: true })}>不上传字幕</button> : null}
    </div>
    <small role="status">{!sourcePath ? "请先选择视频版本" : selected?.disabled ? "本次不上传字幕" : selected?.path ? `字幕文件：${selected.path}`
      : current?.error || (current ? current.path ? `已匹配整季字幕：${current.path}` : existingVideo ? "没有匹配字幕，请选择 SRT 文件" : "没有匹配字幕，将只上传视频" : "正在匹配整季字幕…")}</small>
    <small>仅匹配同一视频的整季字幕；手动选择时请确保时间轴一致。首次上传字幕需在设置中重新授权频道。</small>
    {error ? <p className="warning-banner" role="alert">{error}</p> : null}
  </fieldset>;
}
