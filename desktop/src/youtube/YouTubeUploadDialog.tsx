import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import type { DownloadBatch } from "../download/model";
import type { YouTubePrivacy, YouTubeUploadIntent } from "./types";

type Props = {
  batch: DownloadBatch;
  sourcePath: string;
  onClose: () => void;
  onSubmit: (request: YouTubeUploadIntent) => void;
};

export function YouTubeUploadDialog({ batch, sourcePath, onClose, onSubmit }: Props) {
  const [title, setTitle] = useState(batch.title.slice(0, 100));
  const [description, setDescription] = useState((batch.series.abstract || batch.title).slice(0, 5000));
  const [tags, setTags] = useState(batch.series.category || "短剧");
  const [privacy, setPrivacy] = useState<YouTubePrivacy>("private");
  const [madeForKids, setMadeForKids] = useState(false);
  const [synthetic, setSynthetic] = useState(/ai|人工智能/i.test(`${batch.series.category} ${batch.title}`));
  const [coverPath, setCoverPath] = useState<string | null>(null);
  const [audienceConfirmed, setAudienceConfirmed] = useState(false);
  const [syntheticConfirmed, setSyntheticConfirmed] = useState(false);
  const [publishConfirmed, setPublishConfirmed] = useState(false);
  const canSubmit = title.trim().length > 0 && audienceConfirmed && syntheticConfirmed && publishConfirmed;

  async function chooseCover() {
    const selected = await open({ multiple: false, directory: false, filters: [{ name: "封面图片", extensions: ["jpg", "jpeg", "png", "webp"] }] });
    if (typeof selected === "string") setCoverPath(selected);
  }

  return (
    <div className="dialog-backdrop" role="presentation">
      <section className="merge-dialog youtube-upload-dialog" role="dialog" aria-modal="true" aria-label="上传到 YouTube">
        <header><div><span className="title-marker" /><h2>上传到 YouTube</h2></div><button type="button" className="icon-button" aria-label="关闭" onClick={onClose}>×</button></header>
        <p className="dialog-source">上传文件：{sourcePath}</p>
        <label>标题<input aria-label="YouTube 标题" value={title} maxLength={100} onChange={(event) => setTitle(event.target.value)} /></label>
        <label>简介<textarea aria-label="YouTube 简介" value={description} maxLength={5000} onChange={(event) => setDescription(event.target.value)} /></label>
        <label>标签<input aria-label="YouTube 标签" value={tags} onChange={(event) => setTags(event.target.value)} placeholder="多个标签用逗号分隔" /></label>
        <div className="youtube-form-grid">
          <label>可见性<select aria-label="YouTube 可见性" value={privacy} onChange={(event) => setPrivacy(event.target.value as YouTubePrivacy)}><option value="private">私享</option><option value="unlisted">不公开</option><option value="public">公开</option></select></label>
          <label>儿童受众<select aria-label="儿童受众" value={madeForKids ? "yes" : "no"} onChange={(event) => setMadeForKids(event.target.value === "yes")}><option value="no">不是面向儿童</option><option value="yes">面向儿童</option></select></label>
          <label>合成内容<select aria-label="合成内容" value={synthetic ? "yes" : "no"} onChange={(event) => setSynthetic(event.target.value === "yes")}><option value="no">不包含</option><option value="yes">包含 AI/合成内容</option></select></label>
        </div>
        <div className="cover-picker"><button type="button" className="secondary-button" onClick={() => void chooseCover()}>选择本地封面</button><small>{coverPath || "不设置自定义封面"}</small></div>
        <div className="upload-confirmations">
          <label><input type="checkbox" checked={audienceConfirmed} onChange={(event) => setAudienceConfirmed(event.target.checked)} />我已确认儿童受众设置准确</label>
          <label><input type="checkbox" checked={syntheticConfirmed} onChange={(event) => setSyntheticConfirmed(event.target.checked)} />我已确认合成内容披露准确</label>
          <label><input type="checkbox" checked={publishConfirmed} onChange={(event) => setPublishConfirmed(event.target.checked)} />我确认将此视频发布到所选 YouTube 频道</label>
        </div>
        <footer><button type="button" className="secondary-button" onClick={onClose}>取消</button><button type="button" className="primary-button" disabled={!canSubmit} onClick={() => onSubmit({
          jobId: `youtube-${Date.now()}`,
          filePath: sourcePath,
          coverPath,
          title: title.trim(),
          description,
          tags: tags.split(/[,，]/).map((value) => value.trim()).filter(Boolean),
          categoryId: "24",
          privacyStatus: privacy,
          selfDeclaredMadeForKids: madeForKids,
          containsSyntheticMedia: synthetic,
          audienceConfirmed,
          syntheticMediaConfirmed: syntheticConfirmed,
          publishConfirmed,
        })}>确认上传</button></footer>
      </section>
    </div>
  );
}
