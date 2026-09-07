import { useMemo, useState } from "react";
import type { DownloadBatch } from "../download/model";
import { completedMergeInputs, isCompletedBatch, safeOutputFileName } from "../media/paths";
import type { MergeMode, MergeQuality, MergeSubmitOptions } from "../media/types";

type MergeVideoDialogProps = {
  batch: DownloadBatch;
  onSubmit: (options: MergeSubmitOptions & { inputs: ReturnType<typeof completedMergeInputs> }) => void;
  onClose: () => void;
};

export function MergeVideoDialog({ batch, onSubmit, onClose }: MergeVideoDialogProps) {
  const completed = isCompletedBatch(batch);
  const inputs = useMemo(() => completedMergeInputs(batch), [batch]);
  const [outputName, setOutputName] = useState(safeOutputFileName(batch.title));
  const [mode, setMode] = useState<MergeMode>("auto");
  const [quality, setQuality] = useState<MergeQuality>("high");
  const episodeLookup = useMemo(() => {
    const map = new Map(batch.items.map((item) => [item.episodeIndex, item]));
    return map;
  }, [batch.items]);

  return (
    <div className="media-dialog-backdrop" role="presentation">
      <form
        className="media-dialog"
        role="dialog"
        aria-labelledby="merge-dialog-title"
        onSubmit={(event) => {
          event.preventDefault();
          if (!completed) return;
          onSubmit({
            outputName: safeOutputFileName(outputName),
            mode,
            quality,
            conflictPolicy: "failIfExists",
            inputs,
          });
        }}
      >
        <header>
          <h2 id="merge-dialog-title">合并视频</h2>
          <p>将已完成的剧集按集数顺序合并。默认保留兼容画面的原始画质，并校正音轨。</p>
        </header>
        <section className="media-episode-list" aria-label="合并剧集">
          <h3>包含剧集</h3>
          <ol>
            {inputs.map((input) => {
              const item = episodeLookup.get(input.episodeIndex);
              return (
                <li key={`${input.episodeIndex}-${input.path}`}>
                  <span>第 {input.episodeIndex} 集</span>
                  <small>{item?.episodeTitle || input.path}</small>
                </li>
              );
            })}
          </ol>
        </section>
        <label className="media-field">
          输出文件名
          <input value={outputName} onChange={(event) => setOutputName(event.target.value)} aria-label="输出文件名" />
        </label>
        <label className="media-field">
          合并方式
          <select aria-label="合并方式" value={mode} onChange={(event) => setMode(event.target.value as MergeMode)}>
            <option value="auto">智能合并（推荐）</option>
            <option value="copy">仅无损合并</option>
            <option value="transcode">统一转为 H.264</option>
          </select>
        </label>
        <p className="media-dialog-note">{mode === "copy" ? "保留原始画质；参数或时间轴不兼容时会提示，避免强行拼接。" : "智能模式保留兼容画面，只校正音轨；需转码时按第一集统一尺寸和帧率，保持画面比例。"}</p>
        <label className="media-field">
          转码画质
          <select aria-label="转码画质" value={quality} disabled={mode === "copy"} onChange={(event) => setQuality(event.target.value as MergeQuality)}>
            <option value="high">画质优先</option>
            <option value="balanced">均衡</option>
            <option value="compact">小体积</option>
          </select>
        </label>
        <p className="media-dialog-note">画质设置用于音视频转码；智能模式下兼容画面保持原始画质。</p>
        {!completed ? <p className="media-dialog-note">全部剧集下载完成后才能合并。</p> : null}
        <footer>
          <button type="button" className="secondary-button" onClick={onClose}>取消</button>
          <button type="submit" className="primary-button" disabled={!completed}>开始合并</button>
        </footer>
      </form>
    </div>
  );
}
