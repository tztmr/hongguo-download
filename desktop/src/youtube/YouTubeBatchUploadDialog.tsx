import { DuplicateSeasonPicker, duplicateReasonLabels, hasConfirmedDuplicate, seasonFields, validSeason } from "./duplicateReview";
import { UploadFormatPicker } from "./UploadFormatPicker";
import { useUploadPreferences } from "./uploadPreferences";
import { SubtitlePicker, subtitleRequest, type SubtitleChoice } from "./SubtitlePicker";
import { useEffect, useRef, useState } from "react";
import type { DownloadBatch } from "../download/model";
import { checkYouTubeUpload } from "./commands";
import { YouTubeVideoLink } from "./YouTubeUploadJobs";
import type { YouTubeDuplicateMatch, YouTubePrivacy, YouTubeUploadIntent } from "./types";
import { UploadSourcePicker } from "./UploadSourcePicker";
import { availableUploadSources, preferredUploadSource, type UploadVideoSource } from "./uploadSources";

export type YouTubeBatchUploadSource = { batch: DownloadBatch; sourcePath: string; sourceOptions?: UploadVideoSource[] };

type Props = {
  sources: YouTubeBatchUploadSource[];
  channelId: string;
  onSubmit: (request: YouTubeUploadIntent) => Promise<unknown>;
  onClose: () => void;
  onQueued?: (count: number) => void;
};

type ReviewItem = YouTubeBatchUploadSource & {
  jobId: string;
  selectedSourcePath: string;
  subtitle?: SubtitleChoice;
  title: string;
  season: string;
  description: string;
  tags: string;
  status: "pending" | "checking" | "submitting" | "queued" | "duplicate" | "review" | "skipped" | "error";
  matches: YouTubeDuplicateMatch[];
  error: string;
};

function reviewItem(source: YouTubeBatchUploadSource): ReviewItem {
  const { batch } = source;
  const categories = batch.series.categoryTags?.length
    ? batch.series.categoryTags
    : batch.series.category.split(/[·,，、/|]+/);
  const tags = categories.map((tag) => tag.trim()).filter(Boolean);
  const dramaTitle = batch.series.title.trim() || batch.title.trim();
  return {
    ...source,
    selectedSourcePath: preferredUploadSource(availableUploadSources(source.sourcePath, source.sourceOptions)),
    jobId: `youtube-${crypto.randomUUID()}`,
    title: batch.title.slice(0, 100),
    season: "",
    description: (batch.series.abstract || batch.title).slice(0, 5000),
    tags: [...new Set([...(tags.length ? tags : ["短剧"]), dramaTitle].filter(Boolean))].join(", "),
    status: "pending", matches: [], error: "",
  };
}

export function YouTubeBatchUploadDialog(props: Props) {
  // Equivalent parent rerenders preserve progress; a different target cancels the old session.
  const sessionKey = JSON.stringify([props.channelId, props.sources.map(({ batch, sourcePath, sourceOptions }) =>
    [batch.id, batch.bookId, batch.series.title, sourcePath, sourceOptions])]);
  return <BatchUploadReview key={sessionKey} {...props} />;
}

function BatchUploadReview({ sources, channelId, onSubmit, onClose, onQueued }: Props) {
  const [items, setItems] = useState(() => sources.map(reviewItem));
  const { settings, setSetting } = useUploadPreferences();
  const privacy = settings.privacy;
  const setPrivacy = (value: YouTubePrivacy) => setSetting("privacy", value);
  const categoryId = settings.categoryId;
  const setCategoryId = (value: string) => setSetting("categoryId", value);
  const madeForKids = settings.madeForKids;
  const setMadeForKids = (value: boolean) => setSetting("madeForKids", value);
  const synthetic = settings.synthetic;
  const setSynthetic = (value: boolean) => setSetting("synthetic", value);
  const paidPromotion = settings.paidPromotion;
  const setPaidPromotion = (value: boolean) => setSetting("paidPromotion", value);
  const [audienceConfirmed, setAudienceConfirmed] = useState(true);
  const [syntheticConfirmed, setSyntheticConfirmed] = useState(true);
  const [publishConfirmed, setPublishConfirmed] = useState(true);
  const [busy, setBusy] = useState(false);
  const active = useRef(true);
  const inFlight = useRef(false);
  const queuedIds = useRef(new Set<string>());
  const confirmed = audienceConfirmed && syntheticConfirmed && publishConfirmed && !!channelId.trim();
  const remaining = items.filter((item) => item.status === "pending" || item.status === "error").length;
  const missingChoices = items.filter((item) => item.status !== "queued" && !item.selectedSourcePath).length;

  useEffect(() => {
    active.current = true;
    return () => { active.current = false; };
  }, []);

  useEffect(() => {
    if (!inFlight.current) setItems(current => current.map(item => item.status === "queued" || item.status === "skipped" ? item : { ...item, status: "pending", matches: [], error: "" }));
  }, [settings.uploadFormat]);

  function close() {
    if (!active.current) return;
    active.current = false;
    onClose();
  }

  function update(jobId: string, patch: Partial<ReviewItem>) {
    setItems((current) => current.map((item) => item.jobId === jobId ? { ...item, ...patch } : item));
  }

  function edit(jobId: string, patch: Pick<Partial<ReviewItem>, "title" | "description" | "tags" | "selectedSourcePath" | "subtitle" | "season">) {
    if (inFlight.current || queuedIds.current.has(jobId)) return;
    update(jobId, { ...patch, status: "pending", matches: [], error: "" });
  }

  async function queue(overrideJobId?: string) {
    if (!active.current || inFlight.current || !confirmed) return;
    if (!overrideJobId && missingChoices) return;
    const targets = items.filter((item) => !queuedIds.current.has(item.jobId) && (overrideJobId
      ? item.jobId === overrideJobId && (item.status === "duplicate" || item.status === "review")
      : item.status === "pending" || item.status === "error"));
    if (!targets.length) return;
    if (targets.some((item) => !item.selectedSourcePath)) return;
    inFlight.current = true;
    setBusy(true);
    let queuedCount = 0;
    try {
      for (const item of targets) {
        if (!active.current) break;
        const title = item.title.trim();
        if (!validSeason(item.season)) { update(item.jobId, { status: "error", error: "季数须为 1～999 的整数，或留空自动识别", matches: [] }); continue; }
        if (!title) {
          update(item.jobId, { status: "error", error: "请填写 YouTube 标题", matches: [] });
          continue;
        }
        // Consent applies only to this explicit attempt. Every retry performs a fresh check.
        const allowDuplicate = item.jobId === overrideJobId;
        const request: YouTubeUploadIntent = {
          jobId: item.jobId, uploadFormat: settings.uploadFormat, filePath: item.selectedSourcePath, coverPath: null,
          subtitle: subtitleRequest(item.subtitle, item.selectedSourcePath, settings.subtitleLanguage),
          title, description: item.description,
          tags: item.tags.split(/[,，]/).map((tag) => tag.trim()).filter(Boolean),
          categoryId, privacyStatus: privacy, selfDeclaredMadeForKids: madeForKids,
          containsSyntheticMedia: synthetic, hasPaidProductPlacement: paidPromotion, audienceConfirmed,
          syntheticMediaConfirmed: syntheticConfirmed, publishConfirmed,
          dedup: { channelId, bookId: item.batch.bookId, dramaTitle: item.batch.series.title, ...seasonFields(item.season), allowDuplicate },
        };
        update(item.jobId, { status: "checking", matches: [], error: "" });
        try {
          const matches = await checkYouTubeUpload({ channelId, title, bookId: item.batch.bookId, dramaTitle: item.batch.series.title, ...seasonFields(item.season), uploadFormat: settings.uploadFormat, sourcePath: item.selectedSourcePath });
          if (!active.current) break;
          if (matches.length && !allowDuplicate) {
            update(item.jobId, { status: hasConfirmedDuplicate(matches) ? "duplicate" : "review", matches });
            continue;
          }
          update(item.jobId, { status: "submitting" });
          await onSubmit(request);
          // Record acceptance before releasing the busy guard, even if dismissal occurred.
          queuedIds.current.add(item.jobId);
          queuedCount += 1;
          if (!active.current) break;
          update(item.jobId, { status: "queued" });
        } catch (reason) {
          if (!active.current) break;
          const message = reason && typeof reason === "object" && "message" in reason
            ? String(reason.message) : typeof reason === "string" ? reason : "查重或提交失败，请重试";
          update(item.jobId, { status: "error", error: message, matches: [] });
        }
      }
    } finally {
      inFlight.current = false;
      if (active.current) {
        setBusy(false);
        // Per-attempt count: successful items are never included again on retry.
        if (queuedCount) onQueued?.(queuedCount);
      }
    }
  }

  return (
    <div className="dialog-backdrop" role="presentation">
      <section className="merge-dialog youtube-upload-dialog youtube-batch-upload-dialog" role="dialog" aria-modal="true" aria-label="批量上传到 YouTube">
        <header>
          <div><span className="title-marker" /><h2>批量上传到 YouTube</h2></div>
          <button type="button" className="icon-button" aria-label="关闭" onClick={close}>×</button>
        </header>
        <p>共 {items.length} 部剧。逐项检查当前频道，确认重复才跳过，季数或类型不明的疑似项留待核对；其余项目继续上传。</p>
        <fieldset className="youtube-upload-fields" disabled={busy}>
          <UploadFormatPicker value={settings.uploadFormat} onChange={value => setSetting("uploadFormat", value)} disabled={busy} />
        <div className="youtube-form-grid">
            <label>类别<select aria-label="YouTube 类别" value={categoryId} onChange={(event) => setCategoryId(event.target.value)}><option value="1">电影/动漫</option><option value="24">娱乐</option></select></label>
            <label>可见性<select aria-label="YouTube 可见性" value={privacy} onChange={(event) => setPrivacy(event.target.value as YouTubePrivacy)}><option value="private">私享</option><option value="unlisted">不公开</option><option value="public">公开</option></select></label>
            <label>儿童受众<select aria-label="儿童受众" value={madeForKids ? "yes" : "no"} onChange={(event) => setMadeForKids(event.target.value === "yes")}><option value="no">不是面向儿童</option><option value="yes">面向儿童</option></select></label>
            <label>合成内容<select aria-label="合成内容" value={synthetic ? "yes" : "no"} onChange={(event) => setSynthetic(event.target.value === "yes")}><option value="no">不包含</option><option value="yes">包含 AI/合成内容</option></select></label>
            <label className="youtube-form-wide">付费宣传内容<select aria-label="付费宣传内容" value={paidPromotion ? "yes" : "no"} onChange={(event) => setPaidPromotion(event.target.value === "yes")}><option value="no">否，我的影片不含付費宣傳內容</option><option value="yes">是，我的影片含有付費宣傳內容</option></select></label>
          </div>
          <div className="upload-confirmations">
            <label><input type="checkbox" checked={audienceConfirmed} onChange={(event) => setAudienceConfirmed(event.target.checked)} />我已确认儿童受众设置准确</label>
            <label><input type="checkbox" checked={syntheticConfirmed} onChange={(event) => setSyntheticConfirmed(event.target.checked)} />我已确认合成内容披露准确</label>
            <label><input type="checkbox" checked={publishConfirmed} onChange={(event) => setPublishConfirmed(event.target.checked)} />我确认将这些视频发布到所选 YouTube 频道</label>
          </div>
        </fieldset>
        <div className="youtube-batch-review-list">
          {items.map((item) => (
            <section className="youtube-batch-review-item" role="group" aria-label={item.batch.series.title || item.batch.title} key={item.jobId}>
              <h3>{item.batch.series.title || item.batch.title}</h3>
              <UploadSourcePicker sources={availableUploadSources(item.sourcePath, item.sourceOptions)} value={item.selectedSourcePath}
                disabled={busy || item.status === "queued"} onChange={(path) => edit(item.jobId, { selectedSourcePath: path })} />
              <DuplicateSeasonPicker value={item.season} onChange={season => edit(item.jobId, { season })} disabled={busy || item.status === "queued"} />
              <SubtitlePicker sourcePath={item.selectedSourcePath} value={item.subtitle} onChange={(subtitle) => edit(item.jobId, { subtitle })} language={settings.subtitleLanguage} onLanguageChange={(value) => setSetting("subtitleLanguage", value)} disabled={busy || item.status === "queued"} />
              <fieldset className="youtube-upload-fields" disabled={busy || item.status === "queued"}>
                <label>标题<input aria-label="YouTube 标题" value={item.title} maxLength={100} onChange={(event) => edit(item.jobId, { title: event.target.value })} /></label>
                <label>简介<textarea aria-label="YouTube 简介" value={item.description} maxLength={5000} onChange={(event) => edit(item.jobId, { description: event.target.value })} /></label>
                <label>标签<input aria-label="YouTube 标签" value={item.tags} placeholder="多个标签用逗号分隔" onChange={(event) => edit(item.jobId, { tags: event.target.value })} /></label>
              </fieldset>
              <p role="status">{{ pending: "待检查", checking: "正在查重…", submitting: "正在加入上传队列…", queued: "已加入上传队列", duplicate: "已跳过确认重复项", review: "疑似重复 · 待核对", skipped: "已手动跳过", error: "失败，可重试" }[item.status]}</p>
              {item.status === "error" ? <p className="warning-banner" role="alert">{item.error}</p> : null}
              {item.status === "duplicate" || item.status === "review" ? (
                <div className="youtube-duplicate-warning" role="alert">
                  <strong>{item.status === "duplicate" ? "确认同剧、同季、同类型重复，已跳过本项" : "仅疑似重复，尚未上传；请核对季数与视频类型"}</strong>
                  <ul>{item.matches.map((match, index) => <li key={`${match.videoId}-${index}`}>
                    <span>{match.title}</span>{" · "}
                    <span>{duplicateReasonLabels[match.reason]}</span>{" "}
                    {match.youtubeUrl ? <YouTubeVideoLink url={match.youtubeUrl} /> : <small>本地上传队列中的任务</small>}
                  </li>)}</ul>
                  <button type="button" className="secondary-button" disabled={busy || !confirmed} onClick={() => void queue(item.jobId)}>{item.status === "review" ? "确认是不同内容，上传" : "仍然上传"}</button>
                  {item.status === "review" && <button type="button" className="secondary-button" disabled={busy} onClick={() => update(item.jobId, { status: "skipped" })}>本次不上传</button>}
                </div>
              ) : null}
            </section>
          ))}
        </div>
        <p role="status">已入队 {items.filter((item) => item.status === "queued").length} · 重复跳过 {items.filter((item) => item.status === "duplicate").length} · 待核对 {items.filter((item) => item.status === "review").length} · 手动跳过 {items.filter((item) => item.status === "skipped").length} · 失败 {items.filter((item) => item.status === "error").length}</p>
        {missingChoices ? <p className="source-choice-required" role="status">还有 {missingChoices} 部剧需要选择上传视频版本。</p> : null}
        <footer>
          <button type="button" className="secondary-button" onClick={close}>取消</button>
          <button type="button" className="primary-button" disabled={busy || !confirmed || !remaining || Boolean(missingChoices)} onClick={() => void queue()}>开始批量上传</button>
        </footer>
      </section>
    </div>
  );
}
