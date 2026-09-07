import { useState } from "react";
import type { DefinitionPreference } from "../types";
import type { UseAppSettingsResult } from "./useAppSettings";
import type { YouTubeModel } from "../youtube/types";
import { YouTubeSettings } from "../youtube/YouTubeSettings";
import { MediaModelsSettings } from "./MediaModelsSettings";

export function SettingsPage({ model, youtube }: { model: UseAppSettingsResult; youtube?: YouTubeModel }) {
  const [proxyDraft, setProxyDraft] = useState<string | undefined>(undefined);
  const [mirrorDraft, setMirrorDraft] = useState<string | undefined>(undefined);
  if (model.loading || !model.settings) {
    return <main className="settings-page loading-state">正在加载设置…</main>;
  }
  const settings = model.settings;
  const saveNetworkSettings = () => {
    void model.update({
      downloadProxy: (proxyDraft ?? settings.downloadProxy ?? "").trim(),
      downloadMirror: (mirrorDraft ?? settings.downloadMirror ?? "").trim(),
    });
  };
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
        <p>管理下载偏好、媒体模型与账号配置</p>
      </header>
      <div className="settings-overview" aria-label="当前设置概览">
        <a href="#settings-download"><span>默认画质</span><strong>{resolutions.find((item) => item.value === settings.definition)?.label}</strong><small>新任务自动使用</small></a>
        <a href="#settings-media"><span>媒体组件</span><strong>{model.components.filter((item) => item.installed).length} / {model.components.length} 已就绪</strong><small>音频分离与字幕识别</small></a>
        <a href="#settings-youtube"><span>YouTube 频道</span><strong>{youtube?.channels.find((item) => item.channelId === youtube.activeChannelId)?.title || "尚未连接"}</strong><small>最多 5 个并发上传</small></a>
      </div>
      <div className="settings-layout">
      <nav className="settings-jump-nav" aria-label="设置分类">
        <a href="#settings-download"><span>01</span>下载偏好<small>保存位置与画质</small></a>
        <a href="#settings-notifications"><span>02</span>系统通知<small>任务完成与新剧提醒</small></a>
        <a href="#settings-network"><span>03</span>下载网络<small>代理与镜像</small></a>
        <a href="#settings-media"><span>04</span>媒体处理模型<small>安装与管理组件</small></a>
        {youtube ? <a href="#settings-youtube"><span>05</span>YouTube<small>凭证与频道授权</small></a> : null}
      </nav>
      <div className="settings-content">
      {model.warning ? <div className="warning-banner">{model.warning}</div> : null}

      <section className="settings-section" id="settings-download">
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

      <section className="settings-section" id="settings-notifications">
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

      <section className="settings-section" id="settings-network">
        <div className="settings-section-title">
          <div><h2>下载网络</h2><p>支持大陆直连、公开国内镜像和本机代理；AI 组件下载立即使用新配置</p></div>
        </div>
        <div className="settings-network-form">
          <label>
            <span>代理地址</span>
            <input
              value={proxyDraft ?? settings.downloadProxy ?? ""}
              onChange={(event) => setProxyDraft(event.target.value)}
              placeholder="http://127.0.0.1:7890 或 socks5://127.0.0.1:7890"
              spellCheck={false}
            />
          </label>
          <label>
            <span>国内镜像地址（可选）</span>
            <input
              value={mirrorDraft ?? settings.downloadMirror ?? ""}
              onChange={(event) => setMirrorDraft(event.target.value)}
              placeholder="https://你的镜像域名/ai-components/"
              spellCheck={false}
            />
          </label>
          <div className="settings-network-actions">
            <button type="button" className="primary-button" onClick={saveNetworkSettings}>保存网络设置</button>
            <button type="button" className="secondary-button" onClick={() => { setProxyDraft(""); setMirrorDraft(""); void model.update({ downloadProxy: "", downloadMirror: "" }); }}>恢复直连</button>
          </div>
          <p className="settings-network-help">代理支持 HTTP、HTTPS、SOCKS5。视频/API 下载的代理在重启应用后生效；留空时使用系统代理环境。</p>
        </div>
      </section>

      <MediaModelsSettings model={model} />
      {youtube ? <div id="settings-youtube"><YouTubeSettings model={youtube} /></div> : null}
      </div>
      </div>
    </main>
  );
}
