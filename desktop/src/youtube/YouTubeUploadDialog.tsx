import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import type { DownloadBatch } from "../download/model";
import { checkYouTubeUpload } from "./commands";
import { YouTubeVideoLink } from "./YouTubeUploadJobs";
import type { YouTubeDuplicateMatch, YouTubePrivacy, YouTubeUploadIntent } from "./types";
import { UploadSourcePicker } from "./UploadSourcePicker";
import { availableUploadSources, type UploadVideoSource } from "./uploadSources";

type Props = {
  batch: DownloadBatch;
  sourcePath: string;
  sourceOptions?: UploadVideoSource[];
  channelId: string;
  onClose: () => void;
  onSubmit: (request: YouTubeUploadIntent) => void | Promise<unknown>;
};

const CATEGORY_SEPARATOR = /[·,，、/|]+/;

function youtubeTags(batch: DownloadBatch) {
  const source = batch.series.categoryTags?.length
    ? batch.series.categoryTags
    : batch.series.category.split(CATEGORY_SEPARATOR);
  const tags = source.map((value) => value.trim()).filter((value, index, values) => value && values.indexOf(value) === index);
  const dramaTitle = batch.series.title.trim() || batch.title.trim();
  return [...new Set([...(tags.length ? tags : ["短剧"]), dramaTitle].filter(Boolean))];
}

export function YouTubeUploadDialog({ batch, sourcePath, sourceOptions, channelId, onClose, onSubmit }: Props) {
  const sources = availableUploadSources(sourcePath, sourceOptions);
  const sourceKey = JSON.stringify([batch.id, sourcePath, sources]);
  const [choice, setChoice] = useState({ key: sourceKey, path: "" });
  const selectedSourcePath = sources.length === 1 ? sources[0].path : choice.key === sourceKey ? choice.path : "";
  const [title, setTitle] = useState(batch.title.slice(0, 100));
  const [description, setDescription] = useState((batch.series.abstract || batch.title).slice(0, 5000));
  const [tags, setTags] = useState(youtubeTags(batch).join(", "));
  const [privacy, setPrivacy] = useState<YouTubePrivacy>("private");
  const [categoryId, setCategoryId] = useState("1");
  const [madeForKids, setMadeForKids] = useState(false);
  const [synthetic, setSynthetic] = useState(true);
  const [paidPromotion, setPaidPromotion] = useState(false);
  const [coverPath, setCoverPath] = useState<string | null>(null);
  const [audienceConfirmed, setAudienceConfirmed] = useState(true);
  const [syntheticConfirmed, setSyntheticConfirmed] = useState(true);
  const [publishConfirmed, setPublishConfirmed] = useState(true);
  const canSubmit = Boolean(selectedSourcePath) && title.trim().length > 0 && audienceConfirmed && syntheticConfirmed && publishConfirmed;

  const [phase, setPhase] = useState<"idle" | "checking" | "submitting">("idle");
  const [matches, setMatches] = useState<YouTubeDuplicateMatch[]>([]);
  const [error, setError] = useState("");
  const generation = useRef(0);
  const inFlight = useRef(false);
  const busy = phase !== "idle";
  useEffect(() => {
    generation.current += 1;
    inFlight.current = false;
    setMatches([]);
    setError("");
    setPhase("idle");
    return () => { generation.current += 1; };
  }, [title, channelId, sourcePath, selectedSourcePath, sourceKey]);

  function close() {
    generation.current += 1;
    onClose();
  }

  async function submit(allowDuplicate = false) {
    if (!canSubmit || inFlight.current) return;
    inFlight.current = true;
    const current = ++generation.current;
    setError("");
    setPhase(allowDuplicate ? "submitting" : "checking");
    const request: YouTubeUploadIntent = {
      jobId: `youtube-${Date.now()}`,
      filePath: selectedSourcePath, coverPath, title: title.trim(), description,
      tags: tags.split(/[,，]/).map((value) => value.trim()).filter(Boolean),
      categoryId, privacyStatus: privacy, selfDeclaredMadeForKids: madeForKids,
      containsSyntheticMedia: synthetic, hasPaidProductPlacement: paidPromotion, audienceConfirmed, syntheticMediaConfirmed: syntheticConfirmed, publishConfirmed,
      dedup: { channelId, bookId: batch.bookId, dramaTitle: batch.series.title, allowDuplicate },
    };
    try {
      if (!allowDuplicate) {
        const found = await checkYouTubeUpload({ channelId, title: request.title, bookId: batch.bookId, dramaTitle: batch.series.title });
        if (generation.current !== current) return;
        setMatches(found);
        if (found.length) return;
      }
      setPhase("submitting");
      await onSubmit(request);
    } catch (reason) {
      if (generation.current !== current) return;
      setMatches([]);
      setError(reason && typeof reason === "object" && "message" in reason ? String(reason.message) : "查重或提交失败，请重试");
    } finally {
      if (generation.current === current) { inFlight.current = false; setPhase("idle"); }
    }
  }

  async function chooseCover() {
    const selected = await open({ multiple: false, directory: false, filters: [{ name: "封面图片", extensions: ["jpg", "jpeg", "png", "webp"] }] });
    if (typeof selected === "string") setCoverPath(selected);
  }

  return (
    <div className="dialog-backdrop" role="presentation">
      <section className="merge-dialog youtube-upload-dialog" role="dialog" aria-modal="true" aria-label="上传到 YouTube">
        <header><div><span className="title-marker" /><h2>上传到 YouTube</h2></div><button type="button" className="icon-button" aria-label="关闭" disabled={phase === "submitting"} onClick={close}>×</button></header>
        <UploadSourcePicker sources={sources} value={selectedSourcePath} disabled={busy} onChange={(path) => { if (!busy) setChoice({ key: sourceKey, path }); }} />
        <fieldset className="youtube-upload-fields" disabled={busy}>
        <label>标题<input aria-label="YouTube 标题" value={title} maxLength={100} onChange={(event) => setTitle(event.target.value)} /></label>
        <label>简介<textarea aria-label="YouTube 简介" value={description} maxLength={5000} onChange={(event) => setDescription(event.target.value)} /></label>
        <label>标签<input aria-label="YouTube 标签" value={tags} onChange={(event) => setTags(event.target.value)} placeholder="多个标签用逗号分隔" /></label>
        <div className="youtube-form-grid">
          <label>类别<select aria-label="YouTube 类别" value={categoryId} onChange={(event) => setCategoryId(event.target.value)}><option value="1">电影/动漫</option><option value="24">娱乐</option></select></label>
          <label>可见性<select aria-label="YouTube 可见性" value={privacy} onChange={(event) => setPrivacy(event.target.value as YouTubePrivacy)}><option value="private">私享</option><option value="unlisted">不公开</option><option value="public">公开</option></select></label>
          <label>儿童受众<select aria-label="儿童受众" value={madeForKids ? "yes" : "no"} onChange={(event) => setMadeForKids(event.target.value === "yes")}><option value="no">不是面向儿童</option><option value="yes">面向儿童</option></select></label>
          <label>合成内容<select aria-label="合成内容" value={synthetic ? "yes" : "no"} onChange={(event) => setSynthetic(event.target.value === "yes")}><option value="no">不包含</option><option value="yes">包含 AI/合成内容</option></select></label>
          <label className="youtube-form-wide">付费宣传内容<select aria-label="付费宣传内容" value={paidPromotion ? "yes" : "no"} onChange={(event) => setPaidPromotion(event.target.value === "yes")}><option value="no">否，我的影片不含付費宣傳內容</option><option value="yes">是，我的影片含有付費宣傳內容</option></select></label>
        </div>
        <div className="cover-picker"><button type="button" className="secondary-button" onClick={() => void chooseCover()}>选择本地封面</button><small>{coverPath || "不设置自定义封面"}</small></div>
        <div className="upload-confirmations">
          <label><input type="checkbox" checked={audienceConfirmed} onChange={(event) => setAudienceConfirmed(event.target.checked)} />我已确认儿童受众设置准确</label>
          <label><input type="checkbox" checked={syntheticConfirmed} onChange={(event) => setSyntheticConfirmed(event.target.checked)} />我已确认合成内容披露准确</label>
          <label><input type="checkbox" checked={publishConfirmed} onChange={(event) => setPublishConfirmed(event.target.checked)} />我确认将此视频发布到所选 YouTube 频道</label>
        </div>
        </fieldset>
        {phase === "checking" ? <p role="status">正在检查当前频道的全部影片…</p> : null}
        {error ? <p className="warning-banner" role="alert">{error}</p> : null}
        {matches.length ? <div className="youtube-duplicate-warning" role="alert">
          <strong>{matches.some((match) => match.reason !== "similarTitle") ? "发现重复影片，本次尚未上传" : "发现可能重复的影片，请核对"}</strong>
          <ul>{matches.map((match) => <li key={match.videoId}>
            <span>{match.title} · {{ sameTitle: "标题相同", sameDrama: "同一部剧的上传记录", similarTitle: "剧名相近" }[match.reason]}</span>
            <YouTubeVideoLink url={match.youtubeUrl} />
          </li>)}</ul>
        </div> : null}
        <footer>
          {matches.length ? <>
            <button type="button" className="secondary-button" disabled={busy || !canSubmit} onClick={() => void submit(true)}>{busy ? "正在提交…" : "仍然上传"}</button>
            <button type="button" className="primary-button" disabled={phase === "submitting"} onClick={close}>跳过本次上传</button>
          </> : <>
            <button type="button" className="secondary-button" disabled={phase === "submitting"} onClick={close}>取消</button>
            <button type="button" className="primary-button" disabled={busy || !canSubmit} onClick={() => void submit()}>{phase === "checking" ? "正在查重…" : phase === "submitting" ? "正在提交…" : "确认上传"}</button>
          </>}
        </footer>
      </section>
    </div>
  );
}
