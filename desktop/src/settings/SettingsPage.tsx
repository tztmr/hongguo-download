import { useRef, useState } from "react";
import type { DefinitionPreference } from "../types";
import type { UseAppSettingsResult } from "./useAppSettings";
import type { YouTubeModel } from "../youtube/types";
import { YouTubeSettings } from "../youtube/YouTubeSettings";
import { MediaModelsSettings } from "./MediaModelsSettings";
import { SaveStatus, type SavePhase } from "../components/SaveStatus";
import { errorMessage } from "../errors";

export function SettingsPage({ model, youtube, hidden = false }: { model: UseAppSettingsResult; youtube?: YouTubeModel; hidden?: boolean }) {
  const [proxyDraft, setProxyDraft] = useState<string | undefined>(undefined);
  const [mirrorDraft, setMirrorDraft] = useState<string | undefined>(undefined);
  const [networkPhase, setNetworkPhase] = useState<SavePhase>("idle");
  const [networkMessage, setNetworkMessage] = useState("");
  const [networkSaving, setNetworkSaving] = useState(false);
  const networkLock = useRef(false);
  const networkRevision = useRef(0);
  function editNetwork(field: "proxy" | "mirror", value: string) {
    networkRevision.current += 1;
    if (field === "proxy") setProxyDraft(value); else setMirrorDraft(value);
    setNetworkPhase("dirty"); setNetworkMessage("");
  }
  if (model.loading) {
    return <main hidden={hidden} className="settings-page loading-state">正在加载设置…</main>;
  }
  if (!model.settings) return <main hidden={hidden} className="settings-page"><h1>设置</h1><div className="warning-banner" role="alert">{model.warning || "设置读取失败，请重试"}</div>{model.reload && <button type="button" className="secondary-button" onClick={model.reload}>重新读取设置</button>}</main>;
  const settings = model.settings;
  const saveNetworkSettings = async (reset = false) => {
    if (networkLock.current) return;
    networkLock.current = true;
    if (reset) { networkRevision.current += 1; setProxyDraft(""); setMirrorDraft(""); }
    const submittedRevision = networkRevision.current;
    setNetworkSaving(true); setNetworkMessage("");
    try {
      const result = await model.update({
        downloadProxy: reset ? "" : (proxyDraft ?? settings.downloadProxy ?? "").trim(),
        downloadMirror: reset ? "" : (mirrorDraft ?? settings.downloadMirror ?? "").trim(),
      });
      if (!result.ok) { setNetworkPhase("error"); setNetworkMessage(`保存失败：${result.message}。修改尚未生效，可重试。`); }
      else if (submittedRevision !== networkRevision.current) {
        setNetworkPhase("dirty"); setNetworkMessage("已保存提交时的配置，仍有新的修改未保存。");
      } else { setProxyDraft(undefined); setMirrorDraft(undefined); setNetworkPhase("saved"); }
    } catch (cause) { setNetworkPhase("error"); setNetworkMessage(`保存失败：${errorMessage(cause)}。修改尚未生效，可重试。`); }
    finally { networkLock.current = false; setNetworkSaving(false); }
  };
  const resolutions: Array<{ value: DefinitionPreference; label: string; hint: string }> = [
    { value: "auto", label: "自动最高", hint: "优先 1080p，不可用时自动降档" },
    { value: "1080p", label: "1080p", hint: "优先全高清" },
    { value: "720p", label: "720p", hint: "文件更小，下载更快" },
  ];
  const permissionCopy = {
    granted: "已获得系统通知权限",
    prompt: "首次发送通知时将请求系统权限",
    denied: "通知权限已关闭，请在系统设置中允许红果下载发送通知",
  }[model.notificationPermission];
  return (
    <main hidden={hidden} className="settings-page">
      <header className="settings-header">
        <span>APP SETTINGS</span>
        <h1>设置</h1>
        <p>管理下载偏好、媒体模型与账号配置</p>
        <SaveStatus phase={model.savePhase}>{model.savePhase === "saving" ? "正在保存设置…" : model.savePhase === "error" ? "设置保存失败，未成功的修改已恢复原值，请重试。" : model.savePhase === "saved" ? "设置已保存" : "画质、通知和模型修改后自动保存；下载网络需点击保存。"}</SaveStatus>
      </header>
      <div className="settings-overview" aria-label="当前设置概览">
        <a href="#settings-download"><span>默认画质</span><strong>{resolutions.find((item) => item.value === settings.definition)?.label}</strong><small>新任务自动使用</small></a>
        <a href="#settings-media"><span>媒体组件</span><strong>{model.components.filter((item) => item.installed).length} / {model.components.length} 已就绪</strong><small>音频分离与字幕识别</small></a>
        <a href="#settings-device"><span>设备身份</span><strong>{model.devicesLoading ? "读取中" : model.devicePool ? `${model.devicePool.active_count} 个可用` : "暂不可用"}</strong><small>device_id 与 install_id</small></a>
        <a href="#settings-youtube"><span>YouTube 频道</span><strong>{youtube?.channels.find((item) => item.channelId === youtube.activeChannelId)?.title || "尚未连接"}</strong><small>最多 5 个并发上传</small></a>
      </div>
      <div className="settings-layout">
      <nav className="settings-jump-nav" aria-label="设置分类">
        <a href="#settings-download"><span>01</span>下载偏好<small>保存位置与画质</small></a>
        <a href="#settings-notifications"><span>02</span>系统通知<small>任务完成与新剧提醒</small></a>
        <a href="#settings-network"><span>03</span>下载网络<small>代理与镜像</small></a>
        <a href="#settings-device"><span>04</span>设备身份<small>device_id 与 install_id</small></a>
        <a href="#settings-media"><span>05</span>媒体处理模型<small>安装与管理组件</small></a>
        {youtube ? <a href="#settings-youtube"><span>06</span>YouTube<small>凭证与频道授权</small></a> : null}
      </nav>
      <div className="settings-content">
      {model.warning ? <div className="warning-banner" role="alert">{model.warning}</div> : null}

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
              onChange={(event) => editNetwork("proxy", event.target.value)}
              placeholder="http://127.0.0.1:7890 或 socks5://127.0.0.1:7890"
              spellCheck={false}
            />
          </label>
          <label>
            <span>国内镜像地址（可选）</span>
            <input
              value={mirrorDraft ?? settings.downloadMirror ?? ""}
              onChange={(event) => editNetwork("mirror", event.target.value)}
              placeholder="https://你的镜像域名/ai-components/"
              spellCheck={false}
            />
          </label>
          <div className="settings-network-actions">
            <button type="button" className="primary-button" disabled={networkSaving || (networkPhase !== "dirty" && networkPhase !== "error")} onClick={() => void saveNetworkSettings()}>{networkSaving ? "保存中…" : networkPhase === "error" ? "重试保存网络设置" : "保存网络设置"}</button>
            <button type="button" className="secondary-button" disabled={networkSaving} onClick={() => void saveNetworkSettings(true)}>恢复直连</button>
          </div>
          <SaveStatus phase={networkSaving ? "saving" : networkPhase}>{networkSaving ? "正在保存网络设置…" : networkMessage || (networkPhase === "dirty" ? "网络设置有未保存的修改" : networkPhase === "saved" ? "网络设置已保存" : "编辑后点击保存，成功后应用新配置。")}</SaveStatus>
          <p className="settings-network-help">代理支持 HTTP、HTTPS、SOCKS5。视频/API 下载的代理在重启应用后生效；留空时使用系统代理环境。</p>
        </div>
      </section>

      <section className="settings-section" id="settings-device">
        <div className="settings-section-title">
          <div><h2>设备身份</h2><p>搜索和下载会自动选择可用设备，无需手动选择。device_id 与 install_id 成对使用，失效时自动切换。</p></div>
        </div>
        <div className="device-pool-card">
          <div className="device-pool-toolbar">
            <span>{model.devicesLoading ? "正在读取设备池…" : model.devicePool ? `${model.devicePool.active_count} 个可用设备 / 共 ${model.devicePool.pool_size} 个` : "设备信息暂不可用"}</span>
            <button type="button" className="secondary-button" disabled={model.devicesLoading} onClick={() => void model.refreshDevice()}>
              {model.devicesLoading ? "正在刷新…" : "更新/刷新设备"}
            </button>
          </div>
          {model.devicePool?.devices.length ? (
            <div className="device-pool-list" role="table" aria-label="设备身份列表">
              {model.devicePool.devices.map((device) => (
                <div className="device-pool-row" role="row" key={`${device.device_id}-${device.install_id}`}>
                  <div role="cell"><span>device_id</span><code>{device.device_id}</code></div>
                  <div role="cell"><span>install_id</span><code>{device.install_id}</code></div>
                  <div role="cell"><span>状态</span><strong className={device.status === "active" && !device.expired ? "device-status-active" : "device-status-inactive"}>{device.status === "active" && !device.expired ? "可用" : "不可用"}</strong></div>
                </div>
              ))}
            </div>
          ) : (
            <p className="device-pool-empty">暂无有效设备，搜索时会自动注册；也可点击“更新/刷新设备”提前注册。</p>
          )}
        </div>
      </section>

      <MediaModelsSettings model={model} />
      {youtube ? <div id="settings-youtube"><YouTubeSettings model={youtube} /></div> : null}
      </div>
      </div>
    </main>
  );
}
