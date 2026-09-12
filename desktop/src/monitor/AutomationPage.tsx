import { useState, type ReactNode } from "react";
import { AutomationIcon, CheckIcon } from "../components/icons";
import type { YouTubeChannel } from "../youtube/types";
import "./automation.css";

const STORAGE_KEY = "hongguo.automation.settings-draft.v1";
const defaults = {
  interval: "5", types: ["真人剧", "漫剧", "AI剧"], scope: "today", orientation: "all",
  keywords: "", exclude: "", completeOnly: true, definition: "auto", concurrency: "1",
  separate: true, subtitles: true, subtitleSource: "original", subtitleFormat: "srt", retries: "3",
  channel: "", privacy: "private", title: "{剧名}", description: "{简介}", tags: "{分类标签}, {剧名}",
  coverSource: "source", metadataVersion: 2,
  duplicate: true, deleteEpisodes: true, deleteFinal: true, keepSubtitles: true, minDisk: "20",
  resume: true, notify: true,
};
type Draft = typeof defaults;
type ToggleKey = { [K in keyof Draft]: Draft[K] extends boolean ? K : never }[keyof Draft];
const tabs = ["监听与下载", "媒体处理", "YouTube 上传", "清理与运行"];

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
    result.coverSource = "source";
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
  function update<K extends keyof Draft>(key: K, value: Draft[K]) {
    setDraft((current) => ({ ...current, [key]: value, ...(key === "separate" && !value ? { subtitleSource: "original" } : {}) })); setDirty(true); setMessage("");
  }
  function save() {
    if (!draft.types.length) { setMessage("请至少选择一种监听类型。"); setTab(0); return; }
    if (!draft.title.trim()) { setMessage("请填写上传标题模板。"); setTab(2); return; }
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify({ ...draft, coverSource: "source", metadataVersion: defaults.metadataVersion }));
      setDirty(false); setMessage("设置草稿已保存到本机，尚未启动自动任务。");
    } catch { setMessage("保存失败：本地存储不可用，请重试。"); }
  }
  function toggle(key: ToggleKey, title: string, hint: string) {
    return <label className="auto-toggle"><span><strong>{title}</strong><small>{hint}</small></span><input type="checkbox" role="switch" checked={draft[key]} onChange={(event) => update(key, event.target.checked)} /><i aria-hidden="true" /></label>;
  }
  const stages = [
    ["监听新剧", `每 ${draft.interval} 分钟`, true], ["下载解密", "逐集处理", true],
    ["合并全集", "校验后继续", true], ["分离背景音乐", "保留人声", draft.separate],
    ["提取字幕", draft.subtitleFormat.toUpperCase(), draft.subtitles], ["删除单集", "处理成功后", draft.deleteEpisodes],
    ["YouTube 查重", "重复则跳过", draft.duplicate], ["上传成片", "等待处理完成", true], ["上传后清理", "成功后删除", draft.deleteFinal],
  ] as const;
  return <main className="auto-page">
    <header className="auto-header"><div><div className="auto-eyebrow">AUTOMATION <span>设置预览</span></div><h1>24 小时自动追剧</h1><p>从发现新剧到上传完成，把每一步安排好。</p></div><div className="auto-header-actions"><span className="auto-idle">未启用</span><button className="primary-button" type="button" onClick={save}>保存设置草稿</button></div></header>
    <section className="auto-pipeline" aria-label="自动化流程"><div className="auto-section-heading"><h2><AutomationIcon size={18} />全流程自动处理</h2><span>按顺序执行 · 失败保留现场</span></div><ol>{stages.map(([name, hint, enabled], index) => <li key={name} className={enabled ? "" : "is-skipped"}><span className="auto-step-number">{String(index + 1).padStart(2, "0")}</span><strong>{name}</strong><small>{enabled ? hint : "已跳过"}</small></li>)}</ol></section>
    <div className="auto-preview-note"><span className="auto-note-dot" /><p>当前仅配置流程。保存不会启动下载、上传或删除；全天运行需要后续接入任务调度，并保持电脑与应用运行。</p></div>
    <div className="auto-workspace"><section className="auto-editor"><nav className="auto-tabs" aria-label="自动追剧设置分类">{tabs.map((name, index) => <button type="button" key={name} aria-pressed={tab === index} className={tab === index ? "active" : ""} onClick={() => setTab(index)}><span>0{index + 1}</span>{name}</button>)}</nav>
    <div className="auto-panel">
      {tab === 0 && <><div className="auto-panel-heading"><h2>发现符合条件的新剧</h2><p>先筛选，再加入队列；同一部剧不重复下载。</p></div><div className="auto-field"><span>监听类型</span><div className="auto-chips">{defaults.types.map((type) => <button type="button" key={type} aria-pressed={draft.types.includes(type)} className={draft.types.includes(type) ? "selected" : ""} onClick={() => update("types", draft.types.includes(type) ? draft.types.filter((item) => item !== type) : [...draft.types, type])}><CheckIcon size={14} />{type}</button>)}</div></div>
      <div className="auto-grid"><Field label="检查频率"><select value={draft.interval} onChange={(e) => update("interval", e.target.value)}>{[1, 3, 5, 10, 15, 30].map((n) => <option key={n} value={n}>每 {n} 分钟</option>)}</select></Field><Field label="新剧范围" hint="按北京时间判断当天上线。"><select value={draft.scope} onChange={(e) => update("scope", e.target.value)}><option value="today">仅今天上线</option><option value="new">新剧榜最新收录</option></select></Field><Field label="视频方向"><select value={draft.orientation} onChange={(e) => update("orientation", e.target.value)}><option value="all">全部方向</option><option value="vertical">仅竖屏</option><option value="horizontal">仅横屏</option></select></Field><Field label="下载画质"><select value={draft.definition} onChange={(e) => update("definition", e.target.value)}><option value="auto">自动最高画质</option><option value="1080p">优先 1080p</option><option value="720p">优先 720p</option></select></Field><Field label="包含关键词" hint="逗号分隔，留空表示不限。"><input value={draft.keywords} placeholder="如：都市, 重生" onChange={(e) => update("keywords", e.target.value)} /></Field><Field label="排除关键词"><input value={draft.exclude} placeholder="如：预告, 花絮" onChange={(e) => update("exclude", e.target.value)} /></Field></div>
      {toggle("completeOnly", "仅处理已完结剧目", "未完结的剧目继续观察，完结后再下载全集。")}
      <div className="auto-path"><span>下载目录 · 沿用应用设置</span><code>{saveDir || "尚未设置"}</code>{onOpenSettings && <button className="text-action" type="button" onClick={onOpenSettings}>前往设置修改 →</button>}</div></>}
      {tab === 1 && <><div className="auto-panel-heading"><h2>准备完整成片</h2><p>每个阶段成功后才进入下一步；缺集或文件校验失败时暂停该剧。</p></div><div className="auto-fixed"><CheckIcon size={16} /><div><strong>自动下载、解密与全集合并</strong><small>按集数顺序合并，检查文件、时长和完整性。</small></div><span>必选步骤</span></div>{toggle("separate", "分离背景音乐", "生成保留人声的成片，已成功处理的文件不重复分离。")}{toggle("subtitles", "提取字幕", "生成独立字幕文件，随成片上传到 YouTube。")}
      <div className="auto-grid"><Field label="字幕识别音轨"><select disabled={!draft.subtitles} value={draft.subtitleSource} onChange={(e) => update("subtitleSource", e.target.value)}><option value="original">原始音轨</option><option value="vocal" disabled={!draft.separate}>分离后的人声音轨</option></select></Field><Field label="字幕文件格式"><select disabled={!draft.subtitles} value={draft.subtitleFormat} onChange={(e) => update("subtitleFormat", e.target.value)}><option value="srt">SRT 字幕</option><option value="vtt">VTT 字幕</option></select></Field></div><div className="auto-inline-note">分离模型、字幕模型与计算设备沿用「设置 → 媒体处理模型」。模型未安装时暂停对应任务。</div></>}
      {tab === 2 && <><div className="auto-panel-heading"><h2>查重后上传到指定频道</h2><p>保留上传记录，避免同一部剧反复发布。</p></div><div className="auto-grid"><Field label="目标 YouTube 频道"><select value={draft.channel} onChange={(e) => update("channel", e.target.value)}><option value="">请选择已授权频道</option>{channels.map((channel) => <option key={channel.channelId} value={channel.channelId}>{channel.title}</option>)}{draft.channel && !channels.some((c) => c.channelId === draft.channel) && <option value={draft.channel} disabled>原频道授权不可用，请重新选择</option>}</select></Field><Field label="上传可见性"><select value={draft.privacy} onChange={(e) => update("privacy", e.target.value)}><option value="private">私享</option><option value="unlisted">不公开列出</option><option value="public">公开</option></select></Field></div>{!channels.length && <div className="auto-inline-note">尚无已授权频道。{onOpenSettings && <button type="button" className="text-action" onClick={onOpenSettings}>前往设置连接 YouTube →</button>}</div>}
      {toggle("duplicate", "上传前检查 YouTube 重复", "按目标频道比对剧目记录与视频标题；重复则跳过，查询失败则等待重试。")}
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
      <div className="auto-fixed auto-source-cover" aria-label="封面设置">
        <CheckIcon size={18} />
        <div><strong>封面 · 使用剧目源封面</strong><small>每部剧使用自己的源封面原图，不重新生成、不替换图片。源封面获取失败时保留文件，等待重试。</small></div>
        <span>跟随源封面</span>
      </div>
      </>}
      {tab === 3 && <><div className="auto-panel-heading"><h2>成功后清理，失败时保留</h2><p>清理仅针对本次自动任务生成的文件。</p></div>{toggle("deleteEpisodes", "删除单集视频", "合并校验通过，且已启用的音轨处理、字幕提取全部成功后删除。")}{toggle("deleteFinal", "上传完成后删除本地成片", "视频上传、YouTube 处理及已启用的字幕上传全部成功后清理。")}{toggle("keepSubtitles", "保留字幕文件", "清理成片时保留字幕，方便修改和再次使用。")}
      <div className="auto-grid"><Field label="同时处理剧目数"><select value={draft.concurrency} onChange={(e) => update("concurrency", e.target.value)}>{[1, 2, 3].map((n) => <option key={n} value={n}>{n} 部{n === 1 ? "（建议）" : ""}</option>)}</select></Field><Field label="失败自动重试"><select value={draft.retries} onChange={(e) => update("retries", e.target.value)}>{[0, 1, 3, 5].map((n) => <option key={n} value={n}>{n === 0 ? "不重试" : `最多 ${n} 次`}</option>)}</select></Field><Field label="磁盘剩余空间下限"><select value={draft.minDisk} onChange={(e) => update("minDisk", e.target.value)}>{[10, 20, 50, 100].map((n) => <option key={n} value={n}>{n} GB</option>)}</select></Field></div>{toggle("resume", "重启后恢复未完成任务", "从上次成功的步骤继续，避免重复下载和上传。")}{toggle("notify", "任务结果通知", "整部剧完成或重试后仍失败时发送通知。")}</>}
    </div><footer className="auto-editor-footer"><span role="status">{message || (dirty ? "有未保存的修改" : "设置草稿 · 尚未应用到运行任务")}</span><button className="primary-button" type="button" onClick={save}>保存设置草稿</button></footer></section>
    <aside className="auto-summary"><div className="auto-summary-title"><AutomationIcon size={20} /><h2>运行方案</h2><span>草稿</span></div><dl><div><dt>监听时段</dt><dd>全天 24 小时</dd></div><div><dt>扫描频率</dt><dd>每 {draft.interval} 分钟</dd></div><div><dt>剧目类型</dt><dd>{draft.types.join(" / ") || "尚未选择"}</dd></div><div><dt>同时处理</dt><dd>{draft.concurrency} 部剧</dd></div><div><dt>上传频道</dt><dd>{channels.find((c) => c.channelId === draft.channel)?.title || "尚未选择"}</dd></div><div><dt>字幕上传</dt><dd>{draft.subtitles ? "随成片上传" : "不上传"}</dd></div></dl><div className="auto-summary-rules"><h3>异常处理约定</h3><p><CheckIcon size={14} />缺集或处理失败，保留源文件</p><p><CheckIcon size={14} />查重失败，等待重试</p><p><CheckIcon size={14} />上传失败，不清理本地成片</p><p><CheckIcon size={14} />磁盘不足 {draft.minDisk} GB，暂停下载</p></div><button type="button" className="auto-start" disabled>启动 24 小时自动任务</button><small className="auto-start-hint">设置预览阶段，执行功能待接入</small></aside></div>
  </main>;
}
