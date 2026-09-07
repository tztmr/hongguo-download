import { useState } from "react";
import type { MediaJobScope } from "../media/types";

type Props = {
  kind: "audioSeparation" | "subtitleExtraction";
  hasMergedVideo: boolean;
  title?: string;
  modelName?: string;
  episodeCount?: number;
  onSubmit: (scope: MediaJobScope) => void;
  onClose: () => void;
};

export function MediaScopeDialog({ kind, hasMergedVideo, title, modelName, episodeCount, onSubmit, onClose }: Props) {
  const [scope, setScope] = useState<MediaJobScope>("episodes");
  const audio = kind === "audioSeparation";
  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={(event) => {
      if (event.currentTarget === event.target) onClose();
    }}>
      <section className="merge-dialog media-scope-dialog" role="dialog" aria-modal="true" aria-label={audio ? "分离背景音乐" : "提取字幕"}>
        <header><div><span className="title-marker" /><h2>{audio ? "分离背景音乐" : "提取字幕"}</h2></div><button type="button" className="icon-button" aria-label="关闭" onClick={onClose}>×</button></header>
        {title ? <div className="media-scope-summary"><strong>{title}</strong><span>{episodeCount} 集 · 当前模型 {modelName}</span></div> : null}
        <p>选择处理范围。逐集任务会为每一集生成独立文件。</p>
        <div className="scope-options" role="radiogroup" aria-label="处理范围">
          <label><input type="radio" name="media-scope" checked={scope === "episodes"} onChange={() => setScope("episodes")} />逐集处理</label>
          {hasMergedVideo ? <label><input type="radio" name="media-scope" checked={scope === "merged"} onChange={() => setScope("merged")} />合并视频</label> : null}
        </div>
        {!hasMergedVideo ? <p className="dialog-note">尚无已验证的合并视频，只能按逐集处理。</p> : null}
        {audio ? <p className="warning-banner">音源分离可能有残留或失真，不保证规避 Content ID，也不代替版权授权。</p> : null}
        <footer><button type="button" className="secondary-button" onClick={onClose}>取消</button><button type="button" className="primary-button" onClick={() => onSubmit(scope)}>{audio ? "开始分离" : "开始提取"}</button></footer>
      </section>
    </div>
  );
}
