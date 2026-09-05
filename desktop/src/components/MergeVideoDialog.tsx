import { useMemo, useState } from "react";
import type { DownloadBatch } from "../download/model";
import { completedMergeInputs, isCompletedBatch, safeOutputFileName } from "../media/paths";
import type { MergeConflictPolicy, MergeSubmitOptions } from "../media/types";

type MergeVideoDialogProps = {
  batch: DownloadBatch;
  onSubmit: (options: MergeSubmitOptions & { inputs: ReturnType<typeof completedMergeInputs> }) => void;
  onClose: () => void;
};

export function MergeVideoDialog({ batch, onSubmit, onClose }: MergeVideoDialogProps) {
  const completed = isCompletedBatch(batch);
  const inputs = useMemo(() => completedMergeInputs(batch), [batch]);
  const [outputName, setOutputName] = useState(safeOutputFileName(batch.title));
  const [transcodeH264, setTranscodeH264] = useState(true);
  const [conflictPolicy, setConflictPolicy] = useState<MergeConflictPolicy>("failIfExists");
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
            transcodeH264,
            conflictPolicy,
            inputs,
          });
        }}
      >
        <header>
          <h2 id="merge-dialog-title">合并视频</h2>
          <p>将已完成的剧集按集数顺序合并。默认转为 H.264，且不覆盖已有文件。</p>
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
        <label className="media-check">
          <input type="checkbox" checked={transcodeH264} onChange={(event) => setTranscodeH264(event.target.checked)} />
          转为 H.264
        </label>
        <fieldset className="media-conflict">
          <legend>已有文件</legend>
          <label>
            <input type="radio" name="conflict" checked={conflictPolicy === "failIfExists"} onChange={() => setConflictPolicy("failIfExists")} />
            不覆盖已有文件
          </label>
          <label>
            <input type="radio" name="conflict" checked={conflictPolicy === "overwrite"} onChange={() => setConflictPolicy("overwrite")} />
            覆盖已有文件
          </label>
        </fieldset>
        {!completed ? <p className="media-dialog-note">全部剧集下载完成后才能合并。</p> : null}
        <footer>
          <button type="button" className="secondary-button" onClick={onClose}>取消</button>
          <button type="submit" className="primary-button" disabled={!completed}>开始合并</button>
        </footer>
      </form>
    </div>
  );
}
