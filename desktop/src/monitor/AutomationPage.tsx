import { MediaConcurrencyControl } from "../components/MediaConcurrencyControl";
import { AutomationJobs } from "./AutomationJobs";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { automationRuntime, automationError, usesMediaSlot, type AutomationSecrets, type AutomationSnapshot } from "./automationRuntime";
import { AutomationIcon, CheckIcon } from "../components/icons";
import type { YouTubeChannel } from "../youtube/types";
import "./automation.css";
import { UploadFormatPicker } from "../youtube/UploadFormatPicker";
import type { YouTubeUploadFormat } from "../youtube/types";
import { orderedCoverModels } from "./coverModels";
import { AIStudio, studioDefaults, type AIStudioSettings } from "./AIStudio";
import { fetchCategoryGroups } from "../api";
import type { CategoryGroup, ContentType } from "../types";

const STORAGE_KEY = "hongguo.automation.settings-draft.v1";
const defaults = {
  ...studioDefaults,
  uploadFormat: "auto" as YouTubeUploadFormat,
  firstEpisodeShorts: false,
  interval: "5", types: ["漫剧", "AI剧"], scope: "today", orientation: "all",
  collectRecommend: true, collectNew: true, collectRank: true, collectSearch: true,
  collectPages: "10", recommendDevices: "3",
  keywords: "", categoryIds: [] as string[], exclude: "", completeOnly: true, maxEpisodes: "300", definition: "auto", concurrency: "1",
  separate: true, subtitles: true, subtitleSource: "original", subtitleFormat: "srt", retries: "3",
  channel: "", privacy: "private", title: "{剧名}", description: "{简介}", tags: "{分类标签}, {剧名}",
  aiMetadataApplied: false, nonAiTitle: "{剧名}", nonAiDescription: "{简介}", nonAiTags: "{分类标签}, {剧名}",
  coverSource: "moyuu", metadataSource: "deepseek", metadataVersion: 3,
  textModel: "deepseek-v4-pro", coverModel: "gpt-image-2", category: "24",
  textPrompt: "根据剧名、源简介和分类标签，生成准确、有吸引力的 YouTube 标题、视频描述、标签，并推荐一个可用的视频分类。不要编造剧情。",
  coverPrompt: "根据《{剧名}》的简介：{简介}，设计适合 YouTube 的横版封面。突出剧情冲突与人物情绪，标题清晰可读。",
  duplicate: true, deleteEpisodes: true, deleteFinal: true, keepSubtitles: true, minDisk: "20",
  resume: true, notify: true,
};
type Draft = typeof defaults;
type AutomationCategory = { id: string; apiId: string; name: string; group: string; contentType: ContentType };
type ToggleKey = { [K in keyof Draft]: Draft[K] extends boolean ? K : never }[keyof Draft];
const tabs = ["监听与下载", "媒体处理", "YouTube 上传", "AI 文案与封面", "清理与运行"];
const scopes: Record<string, string> = { all: "不限制", today: "仅今天上线", new: "最新收录与推荐" };
const demoOutcomes: Record<string, { title: string; detail: string }> = {
  fresh: { title: "未发现重复 → 加入下载队列", detail: "YouTube 与本地记录均未命中，下载时保留原始剧名和唯一 ID。" },
  local: { title: "本地已下载 → 跳过该剧", detail: "命中同一源 ID、同一季的已完成下载记录；即使成片已清理，仍保留去重标记。" },
  youtube: { title: "YouTube 已存在 → 跳过该剧", detail: "目标频道确认同源 ID、同季、同成品类型已上传，不再为该成品重复处理。仅标题相似时仍需核对。" },
  pending: { title: "本地任务未完成 → 续接原任务", detail: "已在队列、下载中或失败待重试的同一 ID、同一季不创建第二个任务。" },
  season: { title: "第二季 / 第三季 → 作为新一季继续", detail: "即使源 ID 相同，明确不同季也分别处理。支持第二季、第2季、Season 2、S02；未标季数不默认当作第一季。" },
  uncertain: { title: "仅标题相似 / 旧记录不完整 → 待核对", detail: "不能确定季数或视频类型时，不自动判定重复；先核对原剧名、季数与正片 / Shorts 类型。" },
  error: { title: "查重未完成 → 等待重试", detail: "查询失败不等于没有重复；保留待检查状态，暂不开始下载。" },
};

function normalizeDraft(value: unknown): Draft {
  try {
    if (!value || typeof value !== "object") return { ...defaults };
    const result = { ...defaults };
    for (const key of Object.keys(defaults) as (keyof Draft)[]) {
      const saved = (value as Record<string, unknown>)[key];
      if (key === "types") {
        if (Array.isArray(saved)) result.types = defaults.types.filter((item) => saved.includes(item));
      } else if (key === "categoryIds") {
        if (Array.isArray(saved)) result.categoryIds = saved.filter((item): item is string => typeof item === "string").slice(0, 200);
      } else if (key === "coverModels") {
        if (Array.isArray(saved)) result.coverModels = orderedCoverModels(saved, "");
      } else if (typeof saved === typeof defaults[key]) {
        Object.assign(result, { [key]: saved });
      }
    }
    // Upgrade the first preview's generic defaults without losing custom copy.
    if ((value as Record<string, unknown>).metadataVersion !== defaults.metadataVersion) {
      if (result.title === "{剧名} | 全集完整版") result.title = defaults.title;
      if (!result.description) result.description = defaults.description;
      if (result.tags === "短剧, 全集") result.tags = defaults.tags;
    }
    if ((value as Record<string, unknown>).metadataVersion !== defaults.metadataVersion) {
      result.coverSource = defaults.coverSource;
      result.metadataSource = defaults.metadataSource;
    }
    result.concurrency = "1";
    if (!Object.prototype.hasOwnProperty.call(scopes, result.scope)) result.scope = defaults.scope;
    if (!["source", "moyuu", "jucodex"].includes(result.coverSource)) result.coverSource = defaults.coverSource;
    if (!["template", "deepseek", "jucodex"].includes(result.metadataSource)) result.metadataSource = defaults.metadataSource;
    // Download-time deduplication is required.
    result.duplicate = true;
    if (!["auto", "shorts", "standard"].includes(result.uploadFormat)) result.uploadFormat = "auto";
    if (result.coverModel === "nanobanana") result.coverModel = defaults.coverModel;
    result.coverModels = orderedCoverModels(result.coverModels, result.coverModel);
    if (!["deepseek-v4-pro", "deepseek-v4-flash"].includes(result.textModel)) result.textModel = result.textModel.includes("flash") ? "deepseek-v4-flash" : "deepseek-v4-pro";
    // Manual single-series AI copy must not become the template for other series.
    if (result.aiMetadataApplied) {
      result.title = result.nonAiTitle; result.description = result.nonAiDescription;
      result.tags = result.nonAiTags; result.aiMetadataApplied = false;
    }
    result.metadataVersion = defaults.metadataVersion;
    return result;
  } catch { return { ...defaults }; }
}

function readDraft(): Draft {
  try { return normalizeDraft(JSON.parse(localStorage.getItem(STORAGE_KEY) || "null")); }
  catch { return { ...defaults }; }
}

function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return <label className="auto-field"><span>{label}</span>{children}{hint && <small>{hint}</small>}</label>;
}

function metadataExample(template: string, limit: number) {
  const variables: Record<string, string> = {
    "{剧名}": "示例短剧", "{集数}": "80", "{简介}": "这里会使用这部剧的源简介；没有简介时使用剧名。", "{分类标签}": "都市, 重生",
  };
  return template.replace(/\{(?:剧名|集数|简介|分类标签)\}/g, (key) => variables[key]).slice(0, limit);
}

function splitCategoryText(value: string) {
  return value.split(/[·,，、/|]+/).map((item) => item.trim()).filter(Boolean);
}

function mergeAutomationCategories(groupsByType: Array<{ contentType: ContentType; groups: CategoryGroup[] }>) {
  const result: AutomationCategory[] = [];
  const seen = new Set<string>();
  for (const { contentType, groups } of groupsByType) {
    for (const group of groups) {
      for (const item of group.items) {
        const key = `${contentType}:${item.id}`;
        if (seen.has(key)) continue;
        seen.add(key);
        result.push({ id: key, apiId: item.id, name: item.name, group: group.name, contentType });
      }
    }
  }
  return result;
}

export function AutomationPage({ saveDir, channels = [], onOpenSettings, aiConcurrency, onAIConcurrencyChange, runtimeEnabled = isTauri() }: {
  aiConcurrency?: number; onAIConcurrencyChange?: (value: number) => Promise<void>;
  saveDir: string; channels?: YouTubeChannel[]; onOpenSettings?: () => void; runtimeEnabled?: boolean;
}) {
  const [draft, setDraft] = useState<Draft>(readDraft);
  const [tab, setTab] = useState(0);
  const [dirty, setDirty] = useState(false);
  const [message, setMessage] = useState("");
  const [demo, setDemo] = useState("fresh");
  const [snapshot, setSnapshot] = useState<AutomationSnapshot | null>(null);
  const [loaded, setLoaded] = useState(!runtimeEnabled);
  const [pending, setPending] = useState("");
  const [error, setError] = useState("");
  const [pollError, setPollError] = useState("");
  const [confirmStart, setConfirmStart] = useState(false);
  const [secrets, setSecrets] = useState<AutomationSecrets>({});
  const [categoryOptions, setCategoryOptions] = useState<AutomationCategory[]>([]);
  const [categoryLoading, setCategoryLoading] = useState(false);
  const [categoryError, setCategoryError] = useState("");
  const [categoryOpen, setCategoryOpen] = useState(false);
  const [categoryDraftIds, setCategoryDraftIds] = useState<string[]>([]);
  const busy = useRef(false);
  const revision = useRef(0);
  const mounted = useRef(true);
  const dirtyRef = useRef(false);
  const editRevision = useRef(0);
  useEffect(() => {
    mounted.current = true;
    if (!runtimeEnabled) return;
    let active = true;
    let initialized = false;
    let refreshing = false;
    async function refresh() {
      if (busy.current || refreshing) return;
      refreshing = true;
      const requestRevision = revision.current;
      try {
        const state = await automationRuntime.snapshot();
        if (!active || requestRevision !== revision.current) return;
        setSnapshot(state); setPollError(""); setLoaded(true);
        if (!initialized) {
          if (state.config && !dirtyRef.current) setDraft(normalizeDraft(state.config));
          if (!state.config) setMessage("尚未保存后台配置；已有本地草稿可点击保存迁移。");
          initialized = true;
        }
      } catch (cause) { if (active && requestRevision === revision.current) setPollError(automationError(cause)); }
      finally { refreshing = false; }
    }
    void refresh();
    const timer = window.setInterval(() => void refresh(), 2000);
    return () => { active = false; mounted.current = false; window.clearInterval(timer); };
  }, [runtimeEnabled]);
  useEffect(() => {
    if (!runtimeEnabled) return;
    let active = true;
    setCategoryLoading(true);
    setCategoryError("");
    Promise.all(([
      ["drama", "真人剧"],
      ["manju", "漫剧"],
    ] as const).map(async ([contentType]) => ({ contentType, groups: await fetchCategoryGroups(contentType) })))
      .then((groups) => {
        if (!active) return;
        setCategoryOptions(mergeAutomationCategories(groups));
      })
      .catch(() => {
        if (active) setCategoryError("分类接口暂时不可用，可继续填写自定义分类");
      })
      .finally(() => { if (active) setCategoryLoading(false); });
    return () => { active = false; };
  }, [runtimeEnabled]);
  useEffect(() => {
    if (!categoryOptions.length || draft.categoryIds.length || !draft.keywords.trim()) return;
    const wanted = new Set(splitCategoryText(draft.keywords));
    const ids = categoryOptions.filter((item) => wanted.has(item.name)).map((item) => item.id);
    if (ids.length) setDraft((current) => current.categoryIds.length ? current : { ...current, categoryIds: ids });
  }, [categoryOptions, draft.categoryIds.length, draft.keywords]);
  function markDirty() { editRevision.current += 1; dirtyRef.current = true; setDirty(true); setConfirmStart(false); setMessage(""); }
  function update<K extends keyof Draft>(key: K, value: Draft[K]) {
    // A newly typed key belongs to the provider/base URL shown when it was entered.
    if (["metadataSource", "coverSource", "jucodexBaseUrl"].includes(key) && draft[key] !== value) {
      setSecrets(current => {
        const next = { ...current };
        if (key !== "coverSource") delete next.text;
        if (key !== "metadataSource") delete next.image;
        return next;
      });
    }
    setDraft((current) => ({ ...current, [key]: value, ...(key === "separate" && !value ? { subtitleSource: "original" } : {}), ...(key === "title" ? { nonAiTitle: String(value) } : key === "description" ? { nonAiDescription: String(value) } : key === "tags" ? { nonAiTags: String(value) } : {}) })); markDirty();
  }
  function updateStudio(key: keyof AIStudioSettings, value: string | string[]) {
    if (key === "coverModels") {
      if (Array.isArray(value)) update("coverModels", value);
    } else if (typeof value === "string") update(key, value);
  }
  function categoryLabels(ids: string[]) {
    const labels: string[] = [];
    for (const id of ids) {
      const option = categoryOptions.find((item) => item.id === id);
      if (option && !labels.includes(option.name)) labels.push(option.name);
    }
    return labels;
  }
  function openCategoryPicker() {
    setCategoryDraftIds(draft.categoryIds);
    setCategoryOpen(true);
  }
  function confirmCategories() {
    const labels = categoryLabels(categoryDraftIds);
    setDraft((current) => ({ ...current, categoryIds: categoryDraftIds, keywords: labels.join("，") }));
    markDirty();
    setCategoryOpen(false);
  }
  function toggleCategory(id: string) {
    setCategoryDraftIds((current) => current.includes(id) ? current.filter((item) => item !== id) : [...current, id]);
  }
  async function perform(label: string, command: () => Promise<AutomationSnapshot>) {
    if (busy.current) return false;
    busy.current = true; revision.current += 1; setPending(label); setError("");
    try {
      const state = await command();
      if (mounted.current) { setSnapshot(state); setMessage(`${label}成功。`); }
      return true;
    } catch (cause) { if (mounted.current) setError(automationError(cause)); return false; }
    finally { busy.current = false; revision.current += 1; if (mounted.current) setPending(""); }
  }
  async function save() {
    if (!draft.types.length) { setMessage("请至少选择一种监听类型。"); setTab(0); return; }
    if (!draft.title.trim()) { setMessage("请填写上传标题模板。"); setTab(2); return; }
    const savedRevision = editRevision.current;
    const config = { ...draft, duplicate: true, metadataVersion: defaults.metadataVersion };
    if (runtimeEnabled) {
      if (!loaded || pending) return;
      if (await perform("保存设置", () => automationRuntime.save(config, secrets))) {
        if (savedRevision === editRevision.current) { setSecrets({}); setDirty(false); dirtyRef.current = false; }
        setMessage(savedRevision === editRevision.current ? "设置已保存。已有任务继续使用创建时的配置，新配置供下次启动使用。" : "已保存提交时的设置，仍有新的修改尚未保存。");
      }
    } else {
      try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(config));
        setDirty(false); dirtyRef.current = false; setMessage("浏览器预览草稿已保存，无法启动后台任务。");
      } catch { setMessage("保存失败：本地存储不可用，请重试。"); }
    }
  }
  const mode = snapshot?.mode ?? "stopped";
  const modeText = !runtimeEnabled ? "浏览器预览" : !loaded ? "连接后台中" : ({ stopped: "已停止", running: "运行中", paused: "已暂停" }[mode]);
  const savedChannel = channels.find(channel => channel.channelId === snapshot?.config?.channel)?.title || String(snapshot?.config?.channel || "未选择");
  const savedPrivacy = ({ private: "私享", unlisted: "不公开列出", public: "公开" } as Record<string, string>)[String(snapshot?.config?.privacy)] || "未设置";
  const currentJobs = snapshot?.jobs.filter(job => job.status !== "completed" && job.status !== "skipped" && (!job.config || job.config.channel === snapshot.config?.channel)) || [];
  const activeGroupCount = currentJobs.filter(usesMediaSlot).length;
  const uploadCount = currentJobs.length - activeGroupCount;
  const scanSummary = snapshot?.scanSummary;
  const groupActive = activeGroupCount >= 10;
  const canStart = runtimeEnabled && loaded && !!snapshot?.config && !dirty && !pending && mode === "stopped";
  function timeLabel(value: number) { return value ? new Date(value < 1e12 ? value * 1000 : value).toLocaleString() : "尚无"; }
  function toggle(key: ToggleKey, title: string, hint: string) {
    return <label className="auto-toggle"><span><strong>{title}</strong><small>{hint}</small></span><input type="checkbox" role="switch" checked={draft[key]} onChange={(event) => update(key, event.target.checked)} /><i aria-hidden="true" /></label>;
  }
  const demoOutcome = draft.firstEpisodeShorts && ["youtube", "local"].includes(demo)
    ? { title: "正片已有记录 → 单独核对首集 Shorts", detail: "复用本地首集或继续首集任务；以频道、源唯一 ID、季数和成品类型分别查重。首集也已成功上传时跳过，未完成时仅续接首集，不重复上传正片。" }
    : demoOutcomes[demo];
  const stages = [
    ["监听新剧", `每 ${draft.interval} 分钟`, true], ["YouTube 与本地查重", draft.firstEpisodeShorts ? "正片与首集分别查重" : "重复则跳过", true], ["下载解密", "保留剧名与 ID", true],
    ["合并全集", "校验后继续", true], ["分离背景音乐", "保留人声", draft.separate],
    ["提取字幕", draft.subtitleFormat.toUpperCase(), draft.subtitles],
    ["AI 文案与封面", "按配置生成", draft.metadataSource !== "template" || draft.coverSource !== "source"], ["上传成片", "等待处理完成", true], ["首集 Shorts 引流", "正片成功后 · 独立查重", draft.firstEpisodeShorts], ["删除单集", "全部上传处理成功后", draft.deleteEpisodes], ["上传后清理", "成功后删除", draft.deleteFinal],
  ] as const;
  return <main className="auto-page">
    <header className="auto-header"><div><div className="auto-eyebrow">AUTOMATION <span>{runtimeEnabled ? "后台执行" : "设置预览"}</span></div><h1>24 小时自动追剧</h1><p>从发现新剧到上传完成，把每一步安排好。</p></div><div className="auto-header-actions"><span className="auto-idle">{modeText}</span><button className="primary-button" type="button" disabled={!!pending || !loaded} onClick={() => void save()}>{pending === "保存设置" ? "保存中…" : runtimeEnabled ? "保存设置" : "保存设置草稿"}</button></div></header>
    <details className="auto-pipeline" aria-label="自动化流程"><summary><span><AutomationIcon size={18} />全流程自动处理</span><small>单部按顺序 · 多部流水线并行</small></summary><ol>{stages.map(([name, hint, enabled], index) => <li key={name} className={enabled ? "" : "is-skipped"}><span className="auto-step-number">{String(index + 1).padStart(2, "0")}</span><strong>{name}</strong><small>{enabled ? hint : "已跳过"}</small></li>)}</ol></details>
    <div className="auto-preview-note"><span className="auto-note-dot" /><p>{runtimeEnabled ? "应用需保持运行，切换页面不影响后台。保存与启动分开；暂停或停止后，当前不可中断请求收尾，暂不开始新阶段。退出应用后不再监控，重开按恢复设置继续。" : "浏览器仅预览设置，不能启动、扫描或模拟后台进度。「AI 文案与封面」的手动生成会真实消耗对应服务额度。"}</p></div>
    {runtimeEnabled && <section className="auto-runtime" aria-label="自动追剧后台状态">
      <div className="auto-section-heading"><h2>后台任务 · {modeText}</h2><div className="auto-runtime-actions">
        <button type="button" disabled={!loaded || !!pending || mode !== "running"} onClick={() => void perform("暂停", () => automationRuntime.control("pause"))}>暂停</button>
        <button type="button" disabled={!loaded || !!pending || mode !== "paused"} onClick={() => void perform("继续", () => automationRuntime.control("resume"))}>继续运行</button>
        <button type="button" disabled={!loaded || !!pending || mode === "stopped"} onClick={() => void perform("停止", () => automationRuntime.control("stop"))}>停止</button>
        <button type="button" disabled={!loaded || !!pending || mode !== "running" || groupActive} onClick={() => void perform("立即扫描", () => automationRuntime.control("scan"))}>立即扫描</button>
      </div></div>
      <p className="auto-runtime-meta">已保存目标：{savedChannel} · {savedPrivacy}。上次扫描：{timeLabel(snapshot?.lastScan || 0)}；下次扫描：{mode === "running" ? groupActive ? "媒体名额已满，转入上传后补位" : snapshot?.nextScan ? timeLabel(snapshot.nextScan) : "准备扫描补位" : "等待启动或继续"}。</p>
      <div className="auto-batch-summary"><strong>1 组 · 最多 10 部 · 自动补位</strong><p>媒体处理 {activeGroupCount} 部 · 空位 {Math.max(0, 10 - activeGroupCount)} 部 · {groupActive ? "媒体名额已满" : "按空位继续监听"}</p><p>上传与收尾 {uploadCount} 部 · 独立队列，不占媒体处理名额</p><p>转入上传、完成或跳过一部，立即监听补位。上传时可继续下载、合并和分离背景音乐；CPU / GPU 按可用资源调度。合并视频超过 12 小时自动跳过并保留文件。</p></div>
      {!!scanSummary?.at && <div className="auto-batch-summary"><strong>最近监听结果</strong>{scanSummary.source && <p>{scanSummary.source}{!!scanSummary.deviceRound && ` · 设备轮次 ${scanSummary.deviceRound}`} · 第 {scanSummary.page || 1} 页</p>}<p>本轮已检查 {scanSummary.checked} 条 · 筛选排除 {scanSummary.filtered} 部 · 已有记录 {scanSummary.known} 部 · 新加入 {scanSummary.added} 部</p>{!!scanSummary.buffered && <p>已发现 {scanSummary.buffered} 部候选等待空位</p>}{scanSummary.note && <p>{scanSummary.note}</p>}<p>{groupActive ? "媒体名额已满，释放空位后继续监听。" : scanSummary.more ? "本轮还有来源或分页，按来源轮流寻找符合条件的新剧。" : scanSummary.added === 0 ? "本轮已扫描完，当前筛选下暂无可补入的新剧；按检查频率继续监听。" : "本轮已扫描完，已加入的任务继续处理；按检查频率监听后续新剧。"}</p></div>}
      {(error || pollError) && <p className="auto-runtime-error" role="alert">{error || pollError}</p>}
      {pending && <p className="auto-runtime-meta" role="status">{pending}，请等待…</p>}
      {snapshot?.warning && <p className="auto-runtime-error">{snapshot.warning}</p>}
      {confirmStart && <div className="auto-start-confirm" role="region" aria-label="确认自动任务启动">
        <strong>确认启动自动下载与上传</strong><p>目标频道：{savedChannel}；上传可见性：{savedPrivacy}。范围：{scopes[String(snapshot?.config?.scope)] || String(snapshot?.config?.scope || "")}。启动后将按已保存设置持续处理符合条件的剧目。</p>
        <button type="button" className="primary-button" disabled={!canStart} onClick={() => { setConfirmStart(false); void perform("启动", automationRuntime.start); }}>确认启动</button>
        <button type="button" onClick={() => setConfirmStart(false)}>取消</button>
      </div>}
      <AutomationJobs jobs={snapshot?.jobs || []} loaded={loaded} pending={!!pending} onAction={(id, action) => void perform(action === "retry" ? "重试任务" : action === "skip" ? "跳过任务" : "确认继续", () => automationRuntime.review(id, action))} />
      <details className="auto-runtime-logs"><summary>运行日志（{snapshot?.logs.length || 0}）</summary><ol>{snapshot?.logs.slice(-100).reverse().map((log, index) => <li key={`${log.at}-${index}`}><time>{timeLabel(log.at)}</time>{log.jobId && <code>{log.jobId}</code>}<span>{log.message}</span></li>)}</ol></details>
    </section>}
    <div className={`auto-workspace${tab === 3 ? " is-studio" : ""}`}><section className="auto-editor"><nav className="auto-tabs" aria-label="自动追剧设置分类">{tabs.map((name, index) => <button type="button" key={name} aria-pressed={tab === index} className={tab === index ? "active" : ""} onClick={() => setTab(index)}><span>0{index + 1}</span>{name}</button>)}</nav>
    <div className="auto-panel">
      {tab === 0 && <><div className="auto-panel-heading"><h2>发现符合条件的新剧</h2><p>先筛选，再加入队列；不同季分别识别，同季已完成内容不重复处理。</p></div><div className="auto-field"><span>监听类型</span><div className="auto-chips">{defaults.types.map((type) => <button type="button" key={type} aria-pressed={draft.types.includes(type)} className={draft.types.includes(type) ? "selected" : ""} onClick={() => update("types", draft.types.includes(type) ? draft.types.filter((item) => item !== type) : [...draft.types, type])}><CheckIcon size={14} />{type}</button>)}</div></div>
      <div className="auto-field"><span>采集来源</span><div className="auto-chips" role="group" aria-label="采集来源">{([
        ["collectRecommend", "推荐轮询"], ["collectNew", "新剧采集"], ["collectRank", "榜单采集"], ["collectSearch", "关键词搜索"],
      ] as const).map(([key, label]) => <button type="button" key={key} aria-pressed={draft[key]} className={draft[key] ? "selected" : ""} onClick={() => update(key, !draft[key])}><CheckIcon size={14} />{label}</button>)}</div><small>推荐、新剧、8 个榜单与关键词搜索轮流采集，统一筛选与查重。关键词搜索使用“包含关键词”，留空时不搜索。</small></div>
      <div className="auto-grid"><Field label="推荐设备轮次" hint="默认 3 轮。使用设备池轮换；每轮在同一设备连续翻页，换设备重新取首屏，不额外批量注册设备。"><input type="number" min="1" max="10" step="1" value={draft.recommendDevices} onChange={e => update("recommendDevices", e.target.value)} disabled={!draft.collectRecommend} /></Field><Field label="每个来源最多翻页" hint="默认 10 页。推荐按每台设备计算；重复页自动结束。各来源轮流请求，轮次结束后按检查频率重新采集。"><input type="number" min="1" max="30" step="1" value={draft.collectPages} onChange={e => update("collectPages", e.target.value)} /></Field></div>
      <div className="auto-grid"><Field label="检查频率"><select value={draft.interval} onChange={(e) => update("interval", e.target.value)}>{[1, 3, 5, 10, 15, 30].map((n) => <option key={n} value={n}>每 {n} 分钟</option>)}</select></Field><Field label="剧目范围" hint="仅今天上线按北京时间核对日期；其他选项不限制上线日期。推荐内容仍需通过类型、集数、方向和分类筛选。"><select value={draft.scope} onChange={(e) => update("scope", e.target.value)}>{Object.entries(scopes).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></Field><Field label="视频方向"><select value={draft.orientation} onChange={(e) => update("orientation", e.target.value)}><option value="all">全部方向</option><option value="vertical">仅竖屏</option><option value="horizontal">仅横屏</option></select></Field><Field label="下载画质"><select value={draft.definition} onChange={(e) => update("definition", e.target.value)}><option value="auto">自动最高画质</option><option value="1080p">优先 1080p</option><option value="720p">优先 720p</option></select></Field><Field label="最多集数" hint="默认 300 集；填写 0 表示不限集数。超过上限跳过整部剧，不截取前几集；已开始的任务沿用原设置。"><input type="number" min="0" max="10000" step="1" value={draft.maxEpisodes} onChange={(e) => update("maxEpisodes", e.target.value)} /></Field><Field label="排除分类"><input value={draft.exclude} placeholder="如：预告, 花絮" onChange={(e) => update("exclude", e.target.value)} /></Field></div>
      <div className="auto-category-field">
        <span>API 分类筛选</span>
        <button type="button" className="auto-category-trigger" aria-haspopup="dialog" aria-expanded={categoryOpen} onClick={openCategoryPicker} disabled={categoryLoading && !categoryOptions.length}>
          {categoryLoading ? "正在读取 API 分类…" : draft.categoryIds.length ? `已选 ${draft.categoryIds.length} 个：${categoryLabels(draft.categoryIds).join("、")}` : "点击选择分类标签"}
        </button>
        {categoryOpen && <div className="auto-category-menu" role="dialog" aria-label="选择 API 分类标签">
          <div className="auto-category-menu-heading"><strong>选择分类标签</strong><small>按 API 返回的真人剧 / 漫剧分类多选</small></div>
          {!categoryOptions.length && <p className="auto-category-empty">{categoryError || "暂无分类数据"}</p>}
          {(["drama", "manju"] as const).map((contentType) => {
            const label = contentType === "drama" ? "真人剧" : "漫剧";
            const groups = categoryOptions.filter((item) => item.contentType === contentType).reduce<Record<string, AutomationCategory[]>>((all, item) => {
              (all[item.group] ||= []).push(item);
              return all;
            }, {});
            return <section key={contentType} className="auto-category-source"><h4>{label}</h4>{Object.entries(groups).map(([group, items]) => <div key={group} className="auto-category-group"><strong>{group}</strong><div>{items.map((item) => <label key={item.id}><input type="checkbox" checked={categoryDraftIds.includes(item.id)} onChange={() => toggleCategory(item.id)} />{item.name}</label>)}</div></div>)}</section>;
          })}
          <div className="auto-category-menu-actions"><button type="button" onClick={() => setCategoryOpen(false)}>取消</button><button type="button" className="primary-button" onClick={confirmCategories}>确定标签</button></div>
        </div>}
        <small>分类来自 API；多选后按“确定标签”保存。已选标签会同步到筛选条件，多个分类按任意一个匹配。</small>
        {categoryError && <small className="auto-runtime-error">{categoryError}</small>}
      </div>
      <Field label="分类筛选（可补充）" hint="可补充 API 未返回的标签；只匹配源分类和分类标签，不匹配剧名或简介。"><input value={draft.keywords} placeholder="如：权谋，重生，古代" onChange={(e) => update("keywords", e.target.value)} /></Field>
      {toggle("completeOnly", "仅处理已完结剧目", "未完结的剧目继续观察，完结后再下载全集。")}
      <div className="auto-fixed"><CheckIcon size={16} /><div><strong>监听后先查重 · 确认重复才跳过</strong><small>按频道、源 ID、季数和正片 / Shorts 类型核对。不同季分别处理；仅剧名相似或旧记录信息不足时待核对。保留原始剧名，AI 改标题不改变去重身份。</small></div><span>必选步骤</span></div>
      <div className="auto-path"><span>本地保留规则</span><strong>剧名 + 唯一 ID</strong><code>示例短剧__demo_123456/</code><small>下载目录保留可读剧名和源 ID；独立记录原始剧名、源 ID、下载状态、目标频道与上传结果。清理视频时保留记录，未完成任务可继续。</small></div>
      <section className="auto-dedup-demo" aria-label="查重结果演示"><div className="auto-section-heading"><h3>查重结果演示</h3><span>演示数据 · 未读取本地或 YouTube</span></div>
        <Field label="模拟查重场景"><select value={demo} onChange={(e) => setDemo(e.target.value)}><option value="fresh">两边均无重复</option><option value="local">本地已下载</option><option value="youtube">YouTube 已存在</option><option value="pending">本地任务未完成</option><option value="season">同一作品的第二季 / 第三季</option><option value="uncertain">标题相似或旧记录信息不足</option><option value="error">查重请求失败</option></select></Field>
        <div className="auto-demo-result" role="status"><strong>{demoOutcome.title}</strong><p>{demoOutcome.detail}</p></div>
      </section>
      <div className="auto-path"><span>下载目录 · 沿用应用设置</span><code>{saveDir || "尚未设置"}</code>{onOpenSettings && <button className="text-action" type="button" onClick={onOpenSettings}>前往设置修改 →</button>}</div></>}
      {tab === 1 && <><div className="auto-panel-heading"><h2>准备完整成片</h2><p>每个阶段成功后才进入下一步；缺集或文件校验失败时暂停该剧。</p></div><div className="auto-fixed"><CheckIcon size={16} /><div><strong>自动下载、解密与全集合并</strong><small>按集数顺序合并，检查文件、时长和完整性。</small></div><span>必选步骤</span></div>{toggle("separate", "分离背景音乐", "生成保留人声的成片，已成功处理的文件不重复分离。")}{onAIConcurrencyChange && <MediaConcurrencyControl max={5} value={aiConcurrency} onChange={onAIConcurrencyChange} />}{toggle("subtitles", "提取字幕", "生成独立字幕文件，随成片上传到 YouTube。")}
      <div className="auto-grid"><Field label="字幕识别音轨"><select disabled={!draft.subtitles} value={draft.subtitleSource} onChange={(e) => update("subtitleSource", e.target.value)}><option value="original">原始音轨</option><option value="vocal" disabled={!draft.separate}>分离后的人声音轨</option></select></Field><Field label="字幕文件格式"><select disabled={!draft.subtitles} value={draft.subtitleFormat} onChange={(e) => update("subtitleFormat", e.target.value)}><option value="srt">SRT 字幕</option><option value="vtt">VTT 字幕</option></select></Field></div><div className="auto-inline-note">分离模型、字幕模型与计算设备沿用「设置 → 媒体处理模型」。模型未安装时暂停对应任务。</div></>}
      {tab === 2 && <><div className="auto-panel-heading"><h2>查重后上传到指定频道</h2><p>保留上传记录，避免同一部剧反复发布。</p></div><div className="auto-grid"><Field label="目标 YouTube 频道"><select value={draft.channel} onChange={(e) => update("channel", e.target.value)}><option value="">请选择已授权频道</option>{channels.map((channel) => <option key={channel.channelId} value={channel.channelId}>{channel.title}</option>)}{draft.channel && !channels.some((c) => c.channelId === draft.channel) && <option value={draft.channel} disabled>原频道授权不可用，请重新选择</option>}</select></Field><Field label="上传可见性"><select value={draft.privacy} onChange={(e) => update("privacy", e.target.value)}><option value="private">私享</option><option value="unlisted">不公开列出</option><option value="public">公开</option></select></Field></div>{!channels.length && <div className="auto-inline-note">尚无已授权频道。{onOpenSettings && <button type="button" className="text-action" onClick={onOpenSettings}>前往设置连接 YouTube →</button>}</div>}
      <UploadFormatPicker value={draft.uploadFormat} onChange={value => update("uploadFormat", value)} />
      {toggle("firstEpisodeShorts", "首集额外上传 Shorts 引流", "默认关闭。开启后，正片上传成功再将首集转为 1:1 方形，额外发布一条 Shorts，不替代正片。")}
      {draft.firstEpisodeShorts && <section className="auto-path" aria-label="首集 Shorts 引流方案">
        <span>首集引流方案</span>
        <strong>正片上传成功 → 首集转方形 Shorts → 在 Studio 手动关联</strong>
        <small>完整首集自动转为 1:1 方形，最长边不超过 1080 像素。横屏上下补黑边，竖屏左右补黑边，保持比例，不裁剪人物和字幕。仅时长不超过 3 分钟时上传，超时跳过并提示，不自动截断。</small>
        <small>首集单独生成标题和描述，只写首集实际剧情；AI Key 不可用时使用原剧名、源简介与源封面。正片与首集分别记录上传结果，每个频道、每部剧的每一季最多一条首集 Shorts。失败只重试首集，不重复上传正片。</small>
        <small>首集保留到 Shorts 上传及 YouTube 处理成功后再清理；失败时保留文件。正片已上传的记录不能直接跳过尚未完成的首集任务。</small>
        <small>引流使用 YouTube Studio 的「相关视频」关联同频道正片，需要高级功能权限；YouTube API 不支持自动关联，需手动设置。关联完成前不加入“点击相关视频”的文案。</small>
        {draft.privacy === "private" && <small>当前为私享测试，无法作为引流目标；正式引流需正片公开或不公开列出，Shorts 公开。此开关不改变可见性。</small>}
        {draft.uploadFormat === "shorts" && <small>正片上传类型当前也是 Shorts；若两项使用同一首集文件，只上传一次。要引流到中长视频，请为正片选择普通／中长视频。</small>}
      </section>}

      <div className="auto-inline-note">YouTube 与本地查重已前移至监听之后、下载之前。以原始剧名、源 ID、季数和成品类型关联频道记录；第二季、第三季分别处理，疑似重复留待核对。</div>
      <div className="auto-inline-note">当前文案来源：{draft.metadataSource === "jucodex" ? "Jucodex · 待核验" : draft.metadataSource === "deepseek" ? (draft.textModel === "deepseek-v4-flash" ? "DeepSeek V4 Flash" : "DeepSeek V4 Pro") : "手动模板"}。Key 未填写、失效或无权限时跳过 AI，按原来的标题、简介、标签和封面上传，不等待 AI 重试。下方保留非 AI 上传模板。</div>
      <div className="auto-inline-note">与「下载管理 → YouTube 上传」一致：标题默认使用剧名，视频描述默认使用源简介；没有简介时使用剧名。每部剧分别填入自己的信息。</div>
      <Field label="标题" hint="{剧名} 自动填入当前剧名，可编辑并搭配 {集数}；生成后最多 100 字符。">
        <input aria-label="YouTube 标题" value={draft.title} maxLength={100} onChange={(e) => update("title", e.target.value)} />
      </Field>
      <div className="auto-title-preview"><span>标题示例</span>{metadataExample(draft.title, 100)}</div>
      <Field label="视频描述" hint="{简介} 自动填入当前剧目的源简介，没有简介时使用剧名；生成后最多 5000 字符。">
        <textarea aria-label="YouTube 视频描述" rows={4} maxLength={5000} value={draft.description} placeholder="{简介}" onChange={(e) => update("description", e.target.value)} />
      </Field>
      <div className="auto-title-preview"><span>描述示例</span>{metadataExample(draft.description, 5000) || "不填写视频描述"}</div>
      <Field label="视频标签" hint="默认使用源分类标签和剧名，重复标签合并；无分类时使用“短剧”。多个标签用逗号分隔。">
        <input value={draft.tags} onChange={(e) => update("tags", e.target.value)} />
      </Field>
      <Field label="默认视频分类" hint="AI 模式下由 AI 从 YouTube 可用分类中推荐；无有效推荐时使用此默认分类。视频分类与标签分别设置。"><select value={draft.category} onChange={(e) => update("category", e.target.value)}><option value="24">娱乐</option><option value="1">电影和动画</option><option value="22">人物和博客</option></select></Field>
      <div className="auto-fixed auto-source-cover"><CheckIcon size={18} /><div><strong>封面 · {draft.coverSource === "jucodex" ? "Jucodex · 待核验" : draft.coverSource === "moyuu" ? "Moyuu AI 生成" : "剧目源封面"}</strong><small>在「AI 文案与封面」中配置生成方式。</small></div></div>
      </>}
      <div hidden={tab !== 3}><AIStudio savedKeys={snapshot?.keyStatus} onKeyChange={runtimeEnabled ? (kind, value) => { setSecrets(current => ({ ...current, [kind]: value })); markDirty(); } : undefined} settings={draft} onChange={updateStudio} onApply={(title, description, tags) => { setDraft(current => ({ ...current,
        ...(!current.aiMetadataApplied ? { nonAiTitle: current.title, nonAiDescription: current.description, nonAiTags: current.tags } : {}),
        aiMetadataApplied: true, title, description, tags: tags.join(", ") })); markDirty();
      }} onFallback={() => { setDraft(current => current.aiMetadataApplied ? { ...current, title: current.nonAiTitle, description: current.nonAiDescription, tags: current.nonAiTags, aiMetadataApplied: false } : current); markDirty(); setMessage("AI Key 不可用，已保留或恢复非 AI 上传模板；按原上传方式继续。"); }} /></div>
      {tab === 4 && <><div className="auto-panel-heading"><h2>成功后清理，失败时保留</h2><p>清理仅针对本次自动任务生成的媒体文件，始终保留剧名、唯一 ID 与处理记录作为去重标记。</p></div>{toggle("deleteEpisodes", "删除单集视频", draft.firstEpisodeShorts ? "正片和首集 Shorts、封面与字幕全部成功后清理；失败保留文件。" : "正片上传处理、封面与已启用的字幕全部成功后删除。")}{toggle("deleteFinal", "上传完成后删除本地成片", "全部上传成功后清理；同时启用删除单集时，字幕移至字幕留存，完整清理本任务文件夹。")}{toggle("keepSubtitles", "保留字幕文件", "完整清理任务文件夹时，将字幕移至「自动追剧/字幕留存」，保留去重记录。")}
      <div className="auto-grid"><div className="auto-fixed"><strong>1 组 · 最多 10 部</strong><small>上传使用独立队列；转入上传或跳过后自动补位，合并总时长超过 12 小时自动跳过。</small></div><Field label="快速重试次数" hint="快速重试用完后每 15 分钟自动继续，无需手动点击；暂停和停止会暂停重试。"><select value={draft.retries} onChange={(e) => update("retries", e.target.value)}>{[0, 1, 3, 5].map((n) => <option key={n} value={n}>{n === 0 ? "仅定时重试" : `${n} 次后转为定时重试`}</option>)}</select></Field><Field label="磁盘剩余空间下限"><select value={draft.minDisk} onChange={(e) => update("minDisk", e.target.value)}>{[10, 20, 50, 100].map((n) => <option key={n} value={n}>{n} GB</option>)}</select></Field></div>{toggle("resume", "重启后恢复未完成任务", "从上次成功的步骤继续，避免重复下载和上传。")}{toggle("notify", "任务结果通知", "整部剧完成时发送通知；可恢复错误由后台定时重试。")}</>}
    </div><footer className="auto-editor-footer"><span role="status">{pending ? `${pending}，请等待…` : message || (dirty ? "有未保存的修改；启动前请保存" : runtimeEnabled ? "编辑区为设置草稿；已有任务保留创建时的配置" : "浏览器设置草稿")}</span><button className="primary-button" type="button" disabled={!!pending || !loaded} onClick={() => void save()}>{runtimeEnabled ? "保存设置" : "保存设置草稿"}</button></footer></section>
    <aside className="auto-summary"><div className="auto-summary-title"><AutomationIcon size={20} /><h2>下次启动方案</h2><span>草稿</span></div><dl><div><dt>监听时段</dt><dd>全天 24 小时</dd></div><div><dt>扫描频率</dt><dd>每 {draft.interval} 分钟</dd></div><div><dt>剧目类型</dt><dd>{draft.types.join(" / ") || "尚未选择"}</dd></div><div><dt>剧目范围</dt><dd>{scopes[draft.scope]}</dd></div><div><dt>采集方式</dt><dd>{draft.collectRecommend ? `推荐 ${draft.recommendDevices} 轮 · ` : ""}每来源最多 {draft.collectPages} 页</dd></div><div><dt>剧目集数</dt><dd>{draft.maxEpisodes === "0" ? "不限集数" : `最多 ${draft.maxEpisodes} 集`}</dd></div><div><dt>去重规则</dt><dd>YouTube + 本地记录</dd></div><div><dt>文案 / 封面</dt><dd>{draft.metadataSource === "jucodex" ? "Jucodex" : draft.metadataSource === "deepseek" ? (draft.textModel === "deepseek-v4-flash" ? "DeepSeek V4 Flash" : "DeepSeek V4 Pro") : "手动模板"}<br />{draft.coverSource === "jucodex" ? "Jucodex" : draft.coverSource === "moyuu" ? "Moyuu AI" : "源封面"}</dd></div><div><dt>并发处理</dt><dd>1 组 · 最多 10 部<br /><small>转入上传，媒体名额立即补位</small></dd></div><div><dt>上传频道</dt><dd>{channels.find((c) => c.channelId === draft.channel)?.title || "尚未选择"}</dd></div><div><dt>首集 Shorts</dt><dd>{draft.firstEpisodeShorts ? "额外上传 · 正片成功后" : "关闭 · 仅上传正片"}</dd></div><div><dt>字幕上传</dt><dd>{draft.subtitles ? "随成片上传" : "不上传"}</dd></div></dl><div className="auto-summary-rules"><h3>异常处理约定</h3><p><CheckIcon size={14} />缺集或处理失败，保留源文件</p><p><CheckIcon size={14} />{draft.firstEpisodeShorts ? "正片与首集分别查重，跳过已完成项" : "确认重复才跳过，疑似项待核对"}</p><p><CheckIcon size={14} />清理媒体，保留剧名与 ID 记录</p><p><CheckIcon size={14} />查重失败，等待重试</p><p><CheckIcon size={14} />AI Key 不可用，按原方式上传</p><p><CheckIcon size={14} />上传失败，不清理本地成片</p><p><CheckIcon size={14} />磁盘不足 {draft.minDisk} GB，暂停下载</p></div><button type="button" className="auto-start" disabled={!canStart} onClick={() => setConfirmStart(true)}>启动 24 小时自动任务</button><small className="auto-start-hint">{!runtimeEnabled ? "浏览器预览不能执行后台任务" : dirty ? "有未保存的修改，请先保存" : !snapshot?.config ? "请先保存后台配置" : "启动使用后台已保存的配置"}</small></aside></div>
  </main>;
}
