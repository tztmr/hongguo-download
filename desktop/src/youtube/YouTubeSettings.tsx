import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { YouTubeVideoLink } from "./YouTubeUploadJobs";
import type { YouTubeModel } from "./types";

export function YouTubeSettings({ model }: { model: YouTubeModel }) {
  const [localError, setLocalError] = useState("");
  const activeChannel = model.channels.find(channel => channel.channelId === model.activeChannelId);
  async function run(operation: () => Promise<unknown>) {
    setLocalError("");
    try { await operation(); } catch (reason) {
      setLocalError(reason && typeof reason === "object" && "message" in reason ? String(reason.message) : "操作未完成，请稍后重试");
    }
  }
  const [authorizing, setAuthorizing] = useState(false);
  async function authorize() {
    if (authorizing || model.busy) return;
    setLocalError("");
    setAuthorizing(true);
    try {
      await model.authorize();
    } catch {
      // The model displays the safe error in the warning banner.
    } finally {
      setAuthorizing(false);
    }
  }
  async function chooseCredential() {
    const selected = await open({ multiple: false, directory: false, filters: [{ name: "Google OAuth JSON", extensions: ["json"] }] });
    if (typeof selected === "string") await model.importCredential(selected);
  }

  return (
    <section className="settings-section" aria-label="YouTube 授权">
      <div className="settings-section-title">
        <div><h2>YouTube 授权</h2><p>凭证保存在本机，授权令牌由系统安全存储管理</p></div>
      </div>
      {localError && !model.error ? <div className="warning-banner" role="alert">{localError}</div> : null}
      {model.error ? <div className="warning-banner" role="alert">{model.error.message}</div> : null}
      {authorizing ? <p role="status">正在授权，请在本次打开的浏览器页面完成登录。应用正在等待回调并连接频道，请保持应用打开。</p> : null}
      <div className="settings-path-card youtube-auth-card">
        <div>
          <strong>{model.credential.configured ? `OAuth 已配置 ${model.credential.clientIdSuffix}` : "尚未导入 OAuth 桌面客户端 JSON"}</strong>
          <p>授权包含上传视频、读取频道、管理视频与字幕，以及只读统计权限。已有频道使用数据分析前，若提示权限不足，请重新授权并勾选统计权限。</p>
        </div>
        <div>
          <button type="button" className="secondary-button" disabled={model.busy} onClick={() => void run(chooseCredential)}>导入凭证</button>
          <button type="button" className="primary-button compact" disabled={model.busy || authorizing || !model.credential.configured} onClick={() => void authorize()}>{authorizing ? "授权中…" : "授权频道"}</button>
          <button type="button" className="secondary-button" disabled={model.busy || !model.credential.configured} onClick={() => {
            if (window.confirm("删除本机 OAuth 客户端配置？已保存的频道授权也将不可使用。")) void run(() => model.removeCredential());
          }}>删除凭证</button>
        </div>
      </div>
      <div className="youtube-permission-guide" aria-label="授权与权限状态">
        <p><strong>凭证：</strong>{model.credential.configured ? "已导入" : "待导入桌面客户端 JSON"} · <strong>当前频道：</strong>{activeChannel?.title || "未选择"}</p>
        <p>已连接频道表示保存了授权记录，实际权限以接口检查结果为准。</p>
        {activeChannel && <button type="button" className="secondary-button" disabled={model.busy || authorizing} onClick={() => void authorize()}>重新授权 / 补充权限</button>}
        <details><summary>授权或统计失败怎么办？</summary>
          <ul>
            <li>权限不足或授权过期：重新授权，在浏览器中勾选所需权限，并选择要管理的频道。</li>
            <li>API 未启用：在凭证所属的 Google Cloud 项目启用对应 API，再刷新数据。</li>
            <li>频道无访问权限：确认登录账号拥有该频道的访问权限，并检查所选频道。</li>
            <li>配额已用完：等待配额恢复后重试，重复授权不能恢复配额。</li>
          </ul>
          <div className="yt-inline-actions"><YouTubeVideoLink url="https://console.cloud.google.com/apis/library/youtube.googleapis.com" label="YouTube Data API" /><YouTubeVideoLink url="https://console.cloud.google.com/apis/library/youtubeanalytics.googleapis.com" label="YouTube Analytics API" /></div>
        </details>
      </div>
      {model.channels.length ? (
        <div className="settings-toggle-list youtube-channel-list">
          {model.channels.map((channel) => (
            <article className="settings-component-card" key={channel.channelId}>
              <div className="settings-component-row">
                <label>
                  <input type="radio" name="youtubeChannel" checked={model.activeChannelId === channel.channelId} disabled={model.busy || authorizing} onChange={() => void run(() => model.setChannel(channel.channelId))} />
                  <span><strong>{channel.title}</strong><small>{channel.channelId}</small></span>
                </label>
                <button type="button" className="secondary-button" disabled={model.busy} onClick={() => {
                  if (window.confirm(`撤销“${channel.title}”的 YouTube 授权？`)) void run(() => model.revoke(channel.channelId));
                }}>撤销授权</button>
              </div>
            </article>
          ))}
        </div>
      ) : <p className="settings-empty-copy">导入凭证后，点击“授权频道”绑定上传账号。</p>}
    </section>
  );
}
