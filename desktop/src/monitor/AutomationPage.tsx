import { useState, type ReactNode } from "react";
import { AutomationIcon, CheckIcon } from "../components/icons";
import type { YouTubeChannel } from "../youtube/types";
import "./automation.css";
import { UploadFormatPicker } from "../youtube/UploadFormatPicker";
import type { YouTubeUploadFormat } from "../youtube/types";
import { AIStudio, studioDefaults } from "./AIStudio";

const STORAGE_KEY = "hongguo.automation.settings-draft.v1";
const defaults = {
  ...studioDefaults,
  uploadFormat: "auto" as YouTubeUploadFormat,
  firstEpisodeShorts: false,
  interval: "5", types: ["真人剧", "漫剧", "AI剧"], scope: "today", orientation: "all",
  keywords: "", exclude: "", completeOnly: true, definition: "auto", concurrency: "1",
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
type ToggleKey = { [K in keyof Draft]: Draft[K] extends boolean ? K : never }[keyof Draft];
const tabs = ["监听与下载", "媒体处理", "YouTube 上传", "AI 文案与封面", "清理与运行"];
const scopes: Record<string, string> = { all: "不限制", today: "仅今天上线", new: "新剧榜最新收录" };
const demoOutcomes: Record<string, { title: string; detail: string }> = {
  fresh: { title: "未发现重复 → 加入下载队列", detail: "YouTube 与本地记录均未命中，下载时保留原始剧名和唯一 ID。" },
  local: { title: "本地已下载 → 跳过该剧", detail: "命中同一源 ID、同一季的已完成下载记录；即使成片已清理，仍保留去重标记。" },
  youtube: { title: "YouTube 已存在 → 跳过该剧", detail: "目标频道确认同源 ID、同季、同成品类型已上传，不再为该成品重复处理。仅标题相似时仍需核对。" },
  pending: { title: "本地任务未完成 → 续接原任务", detail: "已在队列、下载中或失败待重试的同一 ID、同一季不创建第二个任务。" },
  season: { title: "第二季 / 第三季 → 作为新一季继续", detail: "即使源 ID 相同，明确不同季也分别处理。支持第二季、第2季、Season 2、S02；未标季数不默认当作第一季。" },
  uncertain: { title: "仅标题相似 / 旧记录不完整 → 待核对", detail: "不能确定季数或视频类型时，不自动判定重复；先核对原剧名、季数与正片 / Shorts 类型。" },
  error: { title: "查重未完成 → 等待重试", detail: "查询失败不等于没有重复；保留待检查状态，暂不开始下载。" },
};

function readDraft(): Draft {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) || "null");
    if (!value || typeof value !== "object") return { ...defaults };
    const result = { ...defaults };
    for (const key of Object.keys(defaults) as (keyof Draft)[]) {
      const saved = (value as Record<string, unknown>)[key];
      if (key === "types") {
        if (Array.isArray(saved)) result.types = defaults.types.filter((item) => saved.includes(item));
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
    if (!Object.prototype.hasOwnProperty.call(scopes, result.scope)) result.scope = defaults.scope;
    if (!["source", "moyuu", "jucodex"].includes(result.coverSource)) result.coverSource = defaults.coverSource;
    if (!["template", "deepseek", "jucodex"].includes(result.metadataSource)) result.metadataSource = defaults.metadataSource;
    // Download-time deduplication is a required step in this preview.
    result.duplicate = true;
    if (!["auto", "shorts", "standard"].includes(result.uploadFormat)) result.uploadFormat = "auto";
    if (result.coverModel === "nanobanana") result.coverModel = defaults.coverModel;
    if (!["deepseek-v4-pro", "deepseek-v4-flash"].includes(result.textModel)) result.textModel = result.textModel.includes("flash") ? "deepseek-v4-flash" : "deepseek-v4-pro";
    // Keys are memory-only, so a reloaded draft must resume with its non-AI templates.
    if (result.aiMetadataApplied) {
      result.title = result.nonAiTitle; result.description = result.nonAiDescription;
      result.tags = result.nonAiTags; result.aiMetadataApplied = false;
    }
    result.metadataVersion = defaults.metadataVersion;
    return result;
  } catch { return { ...defaults }; }
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

export function AutomationPage({ saveDir, channels = [], onOpenSettings }: {
  saveDir: string; channels?: YouTubeChannel[]; onOpenSettings?: () => void;
}) {
  const [draft, setDraft] = useState<Draft>(readDraft);
  const [tab, setTab] = useState(0);
  const [dirty, setDirty] = useState(false);
  const [message, setMessage] = useState("");
  const [demo, setDemo] = useState("fresh");
  function update<K extends keyof Draft>(key: K, value: Draft[K]) {
    setDraft((current) => ({ ...current, [key]: value, ...(key === "separate" && !value ? { subtitleSource: "original" } : {}), ...(key === "title" ? { nonAiTitle: String(value) } : key === "description" ? { nonAiDescription: String(value) } : key === "tags" ? { nonAiTags: String(value) } : {}) })); setDirty(true); setMessage("");
  }
  function save() {
    if (!draft.types.length) { setMessage("请至少选择一种监听类型。"); setTab(0); return; }
    if (!draft.title.trim()) { setMessage("请填写上传标题模板。"); setTab(2); return; }
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify({ ...draft, duplicate: true, metadataVersion: defaults.metadataVersion }));
      setDirty(false); setMessage("设置草稿已保存到本机，尚未启动自动任务。");
    } catch { setMessage("保存失败：本地存储不可用，请重试。"); }
  }
  function toggle(key: ToggleKey, title: string, hint: string) {
    return <label className="auto-toggle"><span><strong>{title}</strong><small>{hint}</small></span><input type="checkbox" role="switch" checked={draft[key]} onChange={(event) => update(key, event.target.checked)} /><i aria-hidden="true" /></label>;
  }
  const demoOutcome = draft.firstEpisodeShorts && ["youtube", "local"].includes(demo)
    ? { title: "正片已有记录 → 单独核对首集 Shorts", detail: "复用本地首集或继续首集任务；以频道、源唯一 ID、季数和成品类型分别查重。首集也已成功上传时跳过，未完成时仅续接首集，不重复上传正片。" }
    : demoOutcomes[demo];
  const stages = [
    ["监听新剧", `每 ${draft.interval} 分钟`, true], ["YouTube 与本地查重", draft.firstEpisodeShorts ? "正片与首集分别查重" : "重复则跳过", true], ["下载解密", "保留剧名与 ID", true],
    ["合并全集", "校验后继续", true], ["分离背景音乐", "保留人声", draft.separate],
    ["提取字幕", draft.subtitleFormat.toUpperCase(), draft.subtitles], ["删除单集", draft.firstEpisodeShorts ? "保留引流用首集" : "处理成功后", draft.deleteEpisodes],
    ["AI 文案与封面", "按配置生成", draft.metadataSource !== "template" || draft.coverSource !== "source"], ["上传成片", "等待处理完成", true], ["首集 Shorts 引流", "正片成功后 · 独立查重", draft.firstEpisodeShorts], ["上传后清理", "成功后删除", draft.deleteFinal],
  ] as const;
  return <main className="auto-page">
    <header className="auto-header"><div><div className="auto-eyebrow">AUTOMATION <span>设置预览</span></div><h1>24 小时自动追剧</h1><p>从发现新剧到上传完成，把每一步安排好。</p></div><div className="auto-header-actions"><span className="auto-idle">未启用</span><button className="primary-button" type="button" onClick={save}>保存设置草稿</button></div></header>
    <section className="auto-pipeline" aria-label="自动化流程"><div className="auto-section-heading"><h2><AutomationIcon size={18} />全流程自动处理</h2><span>按顺序执行 · 失败保留现场</span></div><ol>{stages.map(([name, hint, enabled], index) => <li key={name} className={enabled ? "" : "is-skipped"}><span className="auto-step-number">{String(index + 1).padStart(2, "0")}</span><strong>{name}</strong><small>{enabled ? hint : "已跳过"}</small></li>)}</ol></section>
    <div className="auto-preview-note"><span className="auto-note-dot" /><p>自动追剧仍为流程草稿；保存不会启动任务。「AI 文案与封面」已支持手动真实调用，点击生成将使用对应服务额度。</p></div>
    <div className={`auto-workspace${tab === 3 ? " is-studio" : ""}`}><section className="auto-editor"><nav className="auto-tabs" aria-label="自动追剧设置分类">{tabs.map((name, index) => <button type="button" key={name} aria-pressed={tab === index} className={tab === index ? "active" : ""} onClick={() => setTab(index)}><span>0{index + 1}</span>{name}</button>)}</nav>
    <div className="auto-panel">
      {tab === 0 && <><div className="auto-panel-heading"><h2>发现符合条件的新剧</h2><p>先筛选，再加入队列；不同季分别识别，同季已完成内容不重复处理。</p></div><div className="auto-field"><span>监听类型</span><div className="auto-chips">{defaults.types.map((type) => <button type="button" key={type} aria-pressed={draft.types.includes(type)} className={draft.types.includes(type) ? "selected" : ""} onClick={() => update("types", draft.types.includes(type) ? draft.types.filter((item) => item !== type) : [...draft.types, type])}><CheckIcon size={14} />{type}</button>)}</div></div>
      <div className="auto-grid"><Field label="检查频率"><select value={draft.interval} onChange={(e) => update("interval", e.target.value)}>{[1, 3, 5, 10, 15, 30].map((n) => <option key={n} value={n}>每 {n} 分钟</option>)}</select></Field><Field label="剧目范围" hint="不限制上线日期；仍按所选剧目类型、方向和关键词筛选。当天按北京时间判断。"><select value={draft.scope} onChange={(e) => update("scope", e.target.value)}>{Object.entries(scopes).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></Field><Field label="视频方向"><select value={draft.orientation} onChange={(e) => update("orientation", e.target.value)}><option value="all">全部方向</option><option value="vertical">仅竖屏</option><option value="horizontal">仅横屏</option></select></Field><Field label="下载画质"><select value={draft.definition} onChange={(e) => update("definition", e.target.value)}><option value="auto">自动最高画质</option><option value="1080p">优先 1080p</option><option value="720p">优先 720p</option></select></Field><Field label="包含关键词" hint="逗号分隔，留空表示不限。"><input value={draft.keywords} placeholder="如：都市, 重生" onChange={(e) => update("keywords", e.target.value)} /></Field><Field label="排除关键词"><input value={draft.exclude} placeholder="如：预告, 花絮" onChange={(e) => update("exclude", e.target.value)} /></Field></div>
      {toggle("completeOnly", "仅处理已完结剧目", "未完结的剧目继续观察，完结后再下载全集。")}
      <div className="auto-fixed"><CheckIcon size={16} /><div><strong>监听后先查重 · 确认重复才跳过</strong><small>按频道、源 ID、季数和正片 / Shorts 类型核对。不同季分别处理；仅剧名相似或旧记录信息不足时待核对。保留原始剧名，AI 改标题不改变去重身份。</small></div><span>必选步骤</span></div>
      <div className="auto-path"><span>本地保留规则 · 待接入执行</span><strong>剧名 + 唯一 ID</strong><code>示例短剧__demo_123456/</code><small>下载目录保留可读剧名和源 ID；独立记录原始剧名、源 ID、下载状态、目标频道与上传结果。清理视频时保留记录，未完成任务可继续。</small></div>
      <section className="auto-dedup-demo" aria-label="查重结果演示"><div className="auto-section-heading"><h3>查重结果演示</h3><span>演示数据 · 未读取本地或 YouTube</span></div>
        <Field label="模拟查重场景"><select value={demo} onChange={(e) => setDemo(e.target.value)}><option value="fresh">两边均无重复</option><option value="local">本地已下载</option><option value="youtube">YouTube 已存在</option><option value="pending">本地任务未完成</option><option value="season">同一作品的第二季 / 第三季</option><option value="uncertain">标题相似或旧记录信息不足</option><option value="error">查重请求失败</option></select></Field>
        <div className="auto-demo-result" role="status"><strong>{demoOutcome.title}</strong><p>{demoOutcome.detail}</p></div>
      </section>
      <div className="auto-path"><span>下载目录 · 沿用应用设置</span><code>{saveDir || "尚未设置"}</code>{onOpenSettings && <button className="text-action" type="button" onClick={onOpenSettings}>前往设置修改 →</button>}</div></>}
      {tab === 1 && <><div className="auto-panel-heading"><h2>准备完整成片</h2><p>每个阶段成功后才进入下一步；缺集或文件校验失败时暂停该剧。</p></div><div className="auto-fixed"><CheckIcon size={16} /><div><strong>自动下载、解密与全集合并</strong><small>按集数顺序合并，检查文件、时长和完整性。</small></div><span>必选步骤</span></div>{toggle("separate", "分离背景音乐", "生成保留人声的成片，已成功处理的文件不重复分离。")}{toggle("subtitles", "提取字幕", "生成独立字幕文件，随成片上传到 YouTube。")}
      <div className="auto-grid"><Field label="字幕识别音轨"><select disabled={!draft.subtitles} value={draft.subtitleSource} onChange={(e) => update("subtitleSource", e.target.value)}><option value="original">原始音轨</option><option value="vocal" disabled={!draft.separate}>分离后的人声音轨</option></select></Field><Field label="字幕文件格式"><select disabled={!draft.subtitles} value={draft.subtitleFormat} onChange={(e) => update("subtitleFormat", e.target.value)}><option value="srt">SRT 字幕</option><option value="vtt">VTT 字幕</option></select></Field></div><div className="auto-inline-note">分离模型、字幕模型与计算设备沿用「设置 → 媒体处理模型」。模型未安装时暂停对应任务。</div></>}
      {tab === 2 && <><div className="auto-panel-heading"><h2>查重后上传到指定频道</h2><p>保留上传记录，避免同一部剧反复发布。</p></div><div className="auto-grid"><Field label="目标 YouTube 频道"><select value={draft.channel} onChange={(e) => update("channel", e.target.value)}><option value="">请选择已授权频道</option>{channels.map((channel) => <option key={channel.channelId} value={channel.channelId}>{channel.title}</option>)}{draft.channel && !channels.some((c) => c.channelId === draft.channel) && <option value={draft.channel} disabled>原频道授权不可用，请重新选择</option>}</select></Field><Field label="上传可见性"><select value={draft.privacy} onChange={(e) => update("privacy", e.target.value)}><option value="private">私享</option><option value="unlisted">不公开列出</option><option value="public">公开</option></select></Field></div>{!channels.length && <div className="auto-inline-note">尚无已授权频道。{onOpenSettings && <button type="button" className="text-action" onClick={onOpenSettings}>前往设置连接 YouTube →</button>}</div>}
      <UploadFormatPicker value={draft.uploadFormat} onChange={value => update("uploadFormat", value)} />
      {toggle("firstEpisodeShorts", "首集额外上传 Shorts 引流", "默认关闭。开启后，每部剧在正片上传成功后额外发布一条首集 Shorts，不替代正片。")}
      {draft.firstEpisodeShorts && <section className="auto-path" aria-label="首集 Shorts 引流方案">
        <span>首集引流方案 · 执行功能待接入</span>
        <strong>正片上传成功 → 首集 Shorts → 关联正片</strong>
        <small>使用完整首集，保留原画幅；仅竖版或方形且不超过 3 分钟时上传。超时、横版或首集缺失时跳过并提示，不自动截断。</small>
        <small>首集单独生成标题和描述，只写首集实际剧情；AI Key 不可用时使用原剧名、源简介与源封面。正片与首集分别记录上传结果，每个频道、每部剧的每一季最多一条首集 Shorts。失败只重试首集，不重复上传正片。</small>
        <small>首集保留到 Shorts 上传及 YouTube 处理成功后再清理；失败时保留文件。正片已上传的记录不能直接跳过尚未完成的首集任务。</small>
        <small>引流使用 YouTube Studio 的「相关视频」关联同频道正片，需要高级功能权限；当前尚未接入自动关联。关联完成前不加入“点击相关视频”的文案。</small>
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
      <div hidden={tab !== 3}><AIStudio settings={draft} onChange={update} onApply={(title, description, tags) => { setDraft(current => ({ ...current,
        ...(!current.aiMetadataApplied ? { nonAiTitle: current.title, nonAiDescription: current.description, nonAiTags: current.tags } : {}),
        aiMetadataApplied: true, title, description, tags: tags.join(", ") })); setDirty(true);
      }} onFallback={() => { setDraft(current => current.aiMetadataApplied ? { ...current, title: current.nonAiTitle, description: current.nonAiDescription, tags: current.nonAiTags, aiMetadataApplied: false } : current); setDirty(true); setMessage("AI Key 不可用，已保留或恢复非 AI 上传模板；按原上传方式继续。"); }} /></div>
      {tab === 4 && <><div className="auto-panel-heading"><h2>成功后清理，失败时保留</h2><p>清理仅针对本次自动任务生成的媒体文件，始终保留剧名、唯一 ID 与处理记录作为去重标记。</p></div>{toggle("deleteEpisodes", "删除单集视频", draft.firstEpisodeShorts ? "合并和媒体处理成功后清理其余单集；首集等待 Shorts 上传及处理成功，失败则保留。" : "合并校验通过，且已启用的音轨处理、字幕提取全部成功后删除。")}{toggle("deleteFinal", "上传完成后删除本地成片", "视频上传、YouTube 处理及已启用的字幕上传全部成功后清理。")}{toggle("keepSubtitles", "保留字幕文件", "清理成片时保留字幕，方便修改和再次使用。")}
      <div className="auto-grid"><Field label="同时处理剧目数"><select value={draft.concurrency} onChange={(e) => update("concurrency", e.target.value)}>{[1, 2, 3].map((n) => <option key={n} value={n}>{n} 部{n === 1 ? "（建议）" : ""}</option>)}</select></Field><Field label="失败自动重试"><select value={draft.retries} onChange={(e) => update("retries", e.target.value)}>{[0, 1, 3, 5].map((n) => <option key={n} value={n}>{n === 0 ? "不重试" : `最多 ${n} 次`}</option>)}</select></Field><Field label="磁盘剩余空间下限"><select value={draft.minDisk} onChange={(e) => update("minDisk", e.target.value)}>{[10, 20, 50, 100].map((n) => <option key={n} value={n}>{n} GB</option>)}</select></Field></div>{toggle("resume", "重启后恢复未完成任务", "从上次成功的步骤继续，避免重复下载和上传。")}{toggle("notify", "任务结果通知", "整部剧完成或重试后仍失败时发送通知。")}</>}
    </div><footer className="auto-editor-footer"><span role="status">{message || (dirty ? "有未保存的修改" : "设置草稿 · 尚未应用到运行任务")}</span><button className="primary-button" type="button" onClick={save}>保存设置草稿</button></footer></section>
    <aside className="auto-summary"><div className="auto-summary-title"><AutomationIcon size={20} /><h2>运行方案</h2><span>草稿</span></div><dl><div><dt>监听时段</dt><dd>全天 24 小时</dd></div><div><dt>扫描频率</dt><dd>每 {draft.interval} 分钟</dd></div><div><dt>剧目类型</dt><dd>{draft.types.join(" / ") || "尚未选择"}</dd></div><div><dt>剧目范围</dt><dd>{scopes[draft.scope]}</dd></div><div><dt>去重规则</dt><dd>YouTube + 本地记录</dd></div><div><dt>文案 / 封面</dt><dd>{draft.metadataSource === "jucodex" ? "Jucodex" : draft.metadataSource === "deepseek" ? (draft.textModel === "deepseek-v4-flash" ? "DeepSeek V4 Flash" : "DeepSeek V4 Pro") : "手动模板"}<br />{draft.coverSource === "jucodex" ? "Jucodex" : draft.coverSource === "moyuu" ? "Moyuu AI" : "源封面"}</dd></div><div><dt>同时处理</dt><dd>{draft.concurrency} 部剧</dd></div><div><dt>上传频道</dt><dd>{channels.find((c) => c.channelId === draft.channel)?.title || "尚未选择"}</dd></div><div><dt>首集 Shorts</dt><dd>{draft.firstEpisodeShorts ? "额外上传 · 正片成功后" : "关闭 · 仅上传正片"}</dd></div><div><dt>字幕上传</dt><dd>{draft.subtitles ? "随成片上传" : "不上传"}</dd></div></dl><div className="auto-summary-rules"><h3>异常处理约定</h3><p><CheckIcon size={14} />缺集或处理失败，保留源文件</p><p><CheckIcon size={14} />{draft.firstEpisodeShorts ? "正片与首集分别查重，跳过已完成项" : "确认重复才跳过，疑似项待核对"}</p><p><CheckIcon size={14} />清理媒体，保留剧名与 ID 记录</p><p><CheckIcon size={14} />查重失败，等待重试</p><p><CheckIcon size={14} />AI Key 不可用，按原方式上传</p><p><CheckIcon size={14} />上传失败，不清理本地成片</p><p><CheckIcon size={14} />磁盘不足 {draft.minDisk} GB，暂停下载</p></div><button type="button" className="auto-start" disabled>启动 24 小时自动任务</button><small className="auto-start-hint">设置预览阶段，执行功能待接入</small></aside></div>
  </main>;
}
