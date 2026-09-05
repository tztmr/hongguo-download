import type { DefinitionPreference, DemucsModel, WhisperModel } from "../types";
import type { UseAppSettingsResult } from "./useAppSettings";
import type { YouTubeModel } from "../youtube/types";
import { YouTubeSettings } from "../youtube/YouTubeSettings";

function formatBytes(value?: number) {
  if (value === undefined) return "未知";
  if (value === 0) return "0 B";
  const compact = (amount: number) => amount.toFixed(1).replace(/\.0$/, "");
  if (value < 1024 * 1024) return `${compact(value / 1024)} KB`;
  if (value < 1024 * 1024 * 1024) return `${compact(value / 1024 / 1024)} MB`;
  return `${compact(value / 1024 / 1024 / 1024)} GB`;
}

export function SettingsPage({ model, youtube }: { model: UseAppSettingsResult; youtube?: YouTubeModel }) {
  if (model.loading || !model.settings) {
    return <main className="settings-page loading-state">正在加载设置…</main>;
  }
  const settings = model.settings;
  const resolutions: Array<{ value: DefinitionPreference; label: string; hint: string }> = [
    { value: "auto", label: "自动最高", hint: "优先 1080p，不可用时自动降档" },
    { value: "1080p", label: "1080p", hint: "优先全高清" },
    { value: "720p", label: "720p", hint: "文件更小，下载更快" },
  ];
  const permissionCopy = {
    granted: "已获得 macOS 通知权限",
    prompt: "首次发送通知时将请求 macOS 权限",
    denied: "通知权限已关闭，请在 macOS 系统设置中允许红果下载发送通知",
  }[model.notificationPermission];
  return (
    <main className="settings-page">
      <header className="settings-header">
        <span>APP SETTINGS</span>
        <h1>设置</h1>
        <p>下载配置保存在本机应用目录中</p>
      </header>
      {model.warning ? <div className="warning-banner">{model.warning}</div> : null}

      <section className="settings-section">
        <div className="settings-section-title">
          <div><h2>下载目录</h2><p>新启动的任务使用当前目录</p></div>
        </div>
        <div className="settings-path-card">
          <code>{settings.saveDir}</code>
          <div>
            <button type="button" className="secondary-button" onClick={() => void model.chooseDirectory()}>选择目录</button>
            <button type="button" className="secondary-button" onClick={() => void model.openDirectory()}>打开目录</button>
          </div>
        </div>
      </section>

      <section className="settings-section">
        <div className="settings-section-title"><div><h2>下载分辨率</h2><p>加入队列时锁定，已有任务不变</p></div></div>
        <div className="resolution-options">
          {resolutions.map((item) => (
            <label className={settings.definition === item.value ? "selected" : ""} key={item.value}>
              <input
                type="radio"
                name="definition"
                value={item.value}
                checked={settings.definition === item.value}
                onChange={() => void model.update({ definition: item.value })}
              />
              <strong>{item.label}</strong>
              <span>{item.hint}</span>
            </label>
          ))}
        </div>
      </section>

      <section className="settings-section">
        <div className="settings-section-title"><div><h2>系统通知</h2><p>{permissionCopy}</p></div></div>
        <div className="settings-toggle-list">
          <label>
            <span><strong>下载完成通知</strong><small>每个剧目批次完成时发送一次</small></span>
            <input type="checkbox" checked={settings.notifyDownloadComplete} onChange={(event) => void model.update({ notifyDownloadComplete: event.target.checked })} />
          </label>
          <label>
            <span><strong>新剧通知</strong><small>监听到今日新上线剧目时合并提醒</small></span>
            <input type="checkbox" checked={settings.notifyNewReleases} onChange={(event) => void model.update({ notifyNewReleases: event.target.checked })} />
          </label>
          <label>
            <span><strong>媒体处理结果通知</strong><small>每个合并、分离或字幕任务结束时发送一次</small></span>
            <input type="checkbox" checked={settings.notifyMediaComplete ?? true} onChange={(event) => void model.update({ notifyMediaComplete: event.target.checked })} />
          </label>
          <label>
            <span><strong>YouTube 上传结果通知</strong><small>上传成功、失败或封面部分失败时发送一次</small></span>
            <input type="checkbox" checked={settings.notifyYouTubeResult ?? true} onChange={(event) => void model.update({ notifyYouTubeResult: event.target.checked })} />
          </label>
        </div>
      </section>

      <section className="settings-section">
        <div className="settings-section-title"><div><h2>媒体处理模型</h2><p>AI 运行环境和模型按需下载，不进入基础安装包</p></div></div>
        <div className="resolution-options">
          {(["htdemucs", "htdemucs_ft"] as DemucsModel[]).map((value) => (
            <label className={(settings.demucsModel || "htdemucs") === value ? "selected" : ""} key={value}>
              <input type="radio" name="demucsModel" value={value} checked={settings.demucsModel === value} onChange={() => void model.update({ demucsModel: value })} />
              <strong>{value}</strong>
              <span>{value === "htdemucs" ? "默认人声/伴奏分离" : "更高质量，体积更大"}</span>
            </label>
          ))}
        </div>
        <div className="resolution-options" style={{ marginTop: 12 }}>
          {(["small", "medium"] as WhisperModel[]).map((value) => (
            <label className={(settings.whisperModel || "small") === value ? "selected" : ""} key={value}>
              <input type="radio" name="whisperModel" value={value} checked={settings.whisperModel === value} onChange={() => void model.update({ whisperModel: value })} />
              <strong>Whisper {value}</strong>
              <span>{value === "small" ? "默认字幕识别" : "更准，速度更慢"}</span>
            </label>
          ))}
        </div>
        <div className="settings-toggle-list" style={{ marginTop: 16 }}>
          {model.components.length ? model.components.map((item) => (
            <article key={item.id} className="settings-component-card">
              <div className="settings-component-row">
                <div>
                  <strong>{item.id}</strong>
                  <p className="settings-component-meta">
                    {item.installed ? `已安装 ${item.installedVersion || item.version}` : "未安装"} · 下载 {formatBytes(item.downloadBytes)} · 占用 {formatBytes(item.installedBytes)}
                    {item.stage ? ` · ${item.stage} ${Math.round(item.percent || 0)}%` : ""}
                  </p>
                  {item.installedPath ? <code className="settings-component-path">{item.installedPath}</code> : null}
                </div>
                <div>
                  <button type="button" className="secondary-button" onClick={() => {
                    const approved = window.confirm(`安装 ${item.id}？\n下载：${formatBytes(item.downloadBytes)}\n安装后占用：${formatBytes(item.installedBytes)}`);
                    if (approved) void model.installComponent(item.id);
                  }}>安装 / 重下</button>
                  <button type="button" className="secondary-button" onClick={() => void model.removeComponent(item.id)} disabled={item.inUse} aria-label="删除模型">
                    删除模型
                  </button>
                </div>
              </div>
            </article>
          )) : <p style={{ color: "var(--muted)", fontSize: 12 }}>尚未配置可安装的媒体组件清单</p>}
        </div>
      </section>
      {youtube ? <YouTubeSettings model={youtube} /> : null}
    </main>
  );
}
