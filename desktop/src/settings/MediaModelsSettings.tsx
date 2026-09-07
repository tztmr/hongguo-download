import { useRef, useState } from "react";
import type { AIComponentStatus } from "../types";
import type { UseAppSettingsResult } from "./useAppSettings";
import { DownloadIcon, QueueIcon } from "../components/icons";

const componentNames: Record<string, string> = {
  runtime: "AI 运行环境", "demucs-htdemucs": "标准音频分离模型", "demucs-htdemucs_ft": "高质量音频分离模型",
  "whisper-small": "轻量字幕识别模型", "whisper-medium": "高精度字幕识别模型",
};
const stageNames: Record<string, string> = {
  checking: "检查磁盘空间", downloading: "正在下载", verifying: "校验文件", extracting: "解压组件",
  selfTesting: "运行自检", installed: "安装完成", failed: "安装失败，可重试",
};
function formatBytes(value: number) {
  const units = ["B", "KB", "MB", "GB"];
  const exponent = value > 0 ? Math.min(3, Math.floor(Math.log(value) / Math.log(1024))) : 0;
  return `${(value / 1024 ** exponent).toFixed(1).replace(/\.0$/, "")} ${units[exponent]}`;
}
function isInstalling(item: AIComponentStatus) { return Boolean(item.stage && !["installed", "failed"].includes(item.stage)); }

export function MediaModelsSettings({ model }: { model: UseAppSettingsResult }) {
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [pendingIds, setPendingIds] = useState<string[]>([]);
  const [error, setError] = useState("");
  const installLock = useRef(false);
  const settings = model.settings!;
  const selected = model.components.filter((item) => selectedIds.includes(item.id) && !isInstalling(item) && !item.inUse);
  const totalDownload = selected.reduce((sum, item) => sum + item.downloadBytes, 0);
  const totalInstalled = selected.reduce((sum, item) => sum + item.installedBytes, 0);
  const recommendedIds = ["runtime", `demucs-${settings.demucsModel || "htdemucs"}`, `whisper-${settings.whisperModel || "small"}`];
  const missing = model.components.filter((item) => recommendedIds.includes(item.id) && !item.installed && !item.inUse && !isInstalling(item));
  async function install(items: AIComponentStatus[]) {
    if (!items.length || installLock.current) return;
    const approved = window.confirm(`下载选中的 ${items.length} 个组件？\n${items.map((item) => item.id).join("、")}\n下载：${formatBytes(items.reduce((sum, item) => sum + item.downloadBytes, 0))}\n安装后占用：${formatBytes(items.reduce((sum, item) => sum + item.installedBytes, 0))}`);
    if (!approved) return;
    installLock.current = true;
    setPendingIds(items.map((item) => item.id));
    setError("");
    try {
      for (const item of items) {
        await model.installComponent(item.id);
        setSelectedIds((ids) => ids.filter((id) => id !== item.id));
      }
    } catch (reason) { setError(reason instanceof Error ? reason.message : "组件安装失败，请重试"); }
    finally { installLock.current = false; setPendingIds([]); }
  }
  const ready = (id: string) => model.components.some((item) => item.id === id && item.installed);
  return (
    <section className="settings-section media-model-settings" id="settings-media">
      <div className="settings-section-title settings-component-header"><div><h2>媒体处理模型</h2><p>为音频和字幕选择默认模型，新建任务时使用。</p></div><span className="model-local-badge">本机处理</span></div>
      <fieldset className="model-choice-group"><legend>计算设备 <span>AI</span></legend><p>自动模式会优先使用通过运行测试的 NVIDIA CUDA，失败时改用 CPU。</p>
        <div className="model-options">
          {([
            ["auto", "自动选择计算设备", "推荐；CUDA 可用时使用显卡，否则使用 CPU"],
            ["cpu", "仅使用 CPU", "兼容性最好，处理速度通常较慢"],
            ["cuda", "NVIDIA GPU", "要求已安装匹配的 Windows CUDA 运行环境和驱动"],
          ] as const).map(([value, label, hint]) => <label className={(settings.aiDevice || "auto") === value ? "selected" : ""} key={value}>
            <input type="radio" name="aiDevice" checked={(settings.aiDevice || "auto") === value} onChange={() => void model.update({ aiDevice: value })} />
            <span><strong>{label}</strong><small>{value}</small><em>{hint}</em></span>
          </label>)}
        </div>
      </fieldset>
      <div className="model-choice-grid">
        <fieldset className="model-choice-group"><legend>音频分离 <span>Demucs</span></legend><p>分离人声与背景音乐，生成去背景音乐视频。</p>
          <div className="model-options">
            {(["htdemucs", "htdemucs_ft"] as const).map((value) => <label className={(settings.demucsModel || "htdemucs") === value ? "selected" : ""} key={value}>
              <input type="radio" name="demucsModel" checked={(settings.demucsModel || "htdemucs") === value} onChange={() => void model.update({ demucsModel: value })} />
              <span><strong>{value === "htdemucs" ? "标准模式" : "高质量模式"}</strong><small>{value}</small><em>{value === "htdemucs" ? "速度与效果均衡，适合日常处理" : "分离更细致，处理耗时更长"}</em></span>
              <span className={`model-availability ${ready(`demucs-${value}`) ? "ready" : ""}`}>{ready(`demucs-${value}`) ? "已安装" : "待下载"}</span>
            </label>)}
          </div>
        </fieldset>
        <fieldset className="model-choice-group"><legend>字幕识别 <span>Whisper</span></legend><p>识别音频中的对白，导出 SRT 字幕文件。</p>
          <div className="model-options">
            {(["small", "medium"] as const).map((value) => <label className={(settings.whisperModel || "small") === value ? "selected" : ""} key={value}>
              <input type="radio" name="whisperModel" checked={(settings.whisperModel || "small") === value} onChange={() => void model.update({ whisperModel: value })} />
              <span><strong>{value === "small" ? "轻量模式" : "高精度模式"}</strong><small>Whisper {value}</small><em>{value === "small" ? "识别更快，占用空间更小" : "识别更准确，处理耗时更长"}</em></span>
              <span className={`model-availability ${ready(`whisper-${value}`) ? "ready" : ""}`}>{ready(`whisper-${value}`) ? "已安装" : "待下载"}</span>
            </label>)}
          </div>
        </fieldset>
      </div>
      <div className="model-runtime-note"><QueueIcon size={17} /><p>首次使用需安装 AI 运行环境和对应模型。选择默认模型不会立即下载；下载前会确认体积。</p></div>
      <div className="component-library-heading"><div><h3>组件管理 <span>{model.components.filter((item) => item.installed).length} / {model.components.length} 已安装</span></h3><p>按需下载，也可删除闲置模型来释放空间。</p></div><button type="button" className="text-action accent" disabled={!missing.length || !!pendingIds.length} onClick={() => setSelectedIds(missing.map((item) => item.id))}>选择当前模型所需组件</button></div>
      <div className="component-library">
        {model.components.map((item) => {
          const busy = isInstalling(item) || pendingIds.includes(item.id);
          const percent = Math.round(Number.isFinite(item.percent) ? Math.max(0, Math.min(100, item.percent!)) : 0);
          return <article className="component-library-card" key={item.id}>
            <div className="component-library-row">
              <label className="component-library-select"><input type="checkbox" aria-label={`选择 ${item.id} 下载`} checked={selectedIds.includes(item.id)} disabled={!!pendingIds.length || busy || item.inUse} onChange={() => setSelectedIds((ids) => ids.includes(item.id) ? ids.filter((id) => id !== item.id) : [...ids, item.id])} /><span><strong>{componentNames[item.id] || item.id}</strong><small>{item.id} · v{item.version}</small></span></label>
              <span className={`component-state ${item.stage === "failed" ? "failed" : item.installed ? "ready" : ""}`}>{busy ? (stageNames[item.stage || ""] || "等待安装") : item.inUse ? "使用中" : item.stage === "failed" ? "安装失败" : item.installed ? "已安装" : "未安装"}</span>
              <div className="component-library-actions"><button type="button" className="secondary-button" disabled={!!pendingIds.length || busy || item.inUse} onClick={() => void install([item])}>{busy ? "安装中…" : item.installed ? "重新下载" : "下载"}</button><button type="button" className="text-action" aria-label="删除模型" title={item.inUse ? "任务正在使用此组件" : "删除本机组件"} disabled={item.inUse || !item.installed || busy || !!pendingIds.length} onClick={() => void model.removeComponent(item.id)}>删除模型</button></div>
            </div>
            <div className="component-size-row"><span>下载 <strong>{formatBytes(item.downloadBytes)}</strong></span><span>安装占用 <strong>{formatBytes(item.installedBytes)}</strong></span>{item.id === "runtime" ? <span>所有 AI 任务共用</span> : null}</div>
            {item.stage && item.stage !== "installed" ? <div className={`component-install-progress ${item.stage === "failed" ? "failed" : ""}`}><div className="progress-track" role="progressbar" aria-label={`${item.id} 下载进度`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}><span style={{ width: `${percent}%` }} /></div><small>{stageNames[item.stage] || "处理中"} · {percent}%</small></div> : null}
            {item.installedPath ? <details className="component-location"><summary>安装位置{item.installedVersion ? ` · v${item.installedVersion}` : ""}</summary><code>{item.installedPath}</code></details> : null}
          </article>;
        })}
        {!model.components.length ? <div className="component-empty"><DownloadIcon size={24} /><strong>暂无可下载的组件</strong><p>尚未配置可安装的媒体组件清单</p></div> : null}
      </div>
      {error ? <p className="warning-banner" role="alert">{error}</p> : null}
      <div className="component-download-bar"><div aria-live="polite"><strong>{selected.length ? `已选择 ${selected.length} 个组件` : "选择需要安装的组件"}</strong><small>{selected.length ? `下载 ${formatBytes(totalDownload)} · 安装后占用 ${formatBytes(totalInstalled)}` : "AI 组件按需下载，不包含在基础安装包中"}</small></div><button type="button" className="primary-button" disabled={!selected.length || !!pendingIds.length} onClick={() => void install(selected)}><DownloadIcon size={16} />{pendingIds.length ? "下载中…" : `下载选中组件${selected.length ? `（${selected.length}）` : ""}`}</button></div>
    </section>
  );
}
