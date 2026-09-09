import { useId } from "react";
import type { UploadVideoSource } from "./uploadSources";

const labels = { merged: "合并视频（保留背景音乐）", noBackgroundMusic: "去背景音乐视频" };

export function UploadSourcePicker({ sources, value, onChange, disabled = false }: {
  sources: UploadVideoSource[];
  value: string;
  onChange: (path: string) => void;
  disabled?: boolean;
}) {
  const name = useId();
  return (
    <fieldset className="upload-source-picker" disabled={disabled}>
      <legend>上传视频版本</legend>
      {sources.length > 1 ? <>
        <p>检测到两个视频版本，请选择本次上传的版本。</p>
        {sources.map((source) => <label className={`upload-source-option ${value === source.path ? "selected" : ""}`} key={source.path}>
          <input type="radio" name={name} aria-label={labels[source.kind]} checked={value === source.path} onChange={() => onChange(source.path)} />
          <span><strong>{labels[source.kind]}</strong><small>{source.path}</small></span>
        </label>)}
      </> : <p>将上传：{labels[sources[0].kind]}。当前仅此版本可用。</p>}
      {value ? <p className="dialog-source" title={value}>上传文件：{value}</p> : <p className="source-choice-required">请先选择视频版本，再确认上传。</p>}
    </fieldset>
  );
}
