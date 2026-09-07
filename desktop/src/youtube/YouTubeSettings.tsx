import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import type { YouTubeModel } from "./types";

export function YouTubeSettings({ model }: { model: YouTubeModel }) {
  const [authorizing, setAuthorizing] = useState(false);
  async function authorize() {
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
        <div><h2>YouTube 授权</h2><p>凭证仅私密复制到本机，刷新令牌保存在 macOS 钥匙串</p></div>
      </div>
      {model.error ? <div className="warning-banner" role="alert">{model.error.message}</div> : null}
      {authorizing ? <p role="status">正在授权，请在本次打开的浏览器页面完成登录。应用正在等待回调并连接频道，请保持应用打开。</p> : null}
      <div className="settings-path-card youtube-auth-card">
        <div>
          <strong>{model.credential.configured ? `OAuth 已配置 ${model.credential.clientIdSuffix}` : "尚未导入 OAuth 桌面客户端 JSON"}</strong>
          <p>请求上传视频和读取 YouTube 账号权限，用于绑定频道；请在系统浏览器中确认授权。</p>
        </div>
        <div>
          <button type="button" className="secondary-button" disabled={model.busy} onClick={() => void chooseCredential()}>导入凭证</button>
          <button type="button" className="primary-button compact" disabled={model.busy || authorizing || !model.credential.configured} onClick={() => void authorize()}>{authorizing ? "授权中…" : "授权频道"}</button>
          <button type="button" className="secondary-button" disabled={model.busy || !model.credential.configured} onClick={() => {
            if (window.confirm("删除本机 OAuth 客户端配置？已保存的频道授权也将不可使用。")) void model.removeCredential();
          }}>删除凭证</button>
        </div>
      </div>
      {model.channels.length ? (
        <div className="settings-toggle-list youtube-channel-list">
          {model.channels.map((channel) => (
            <article className="settings-component-card" key={channel.channelId}>
              <div className="settings-component-row">
                <label>
                  <input type="radio" name="youtubeChannel" checked={model.activeChannelId === channel.channelId} onChange={() => void model.setChannel(channel.channelId)} />
                  <span><strong>{channel.title}</strong><small>{channel.channelId}</small></span>
                </label>
                <button type="button" className="secondary-button" disabled={model.busy} onClick={() => {
                  if (window.confirm(`撤销“${channel.title}”的 YouTube 授权？`)) void model.revoke(channel.channelId);
                }}>撤销授权</button>
              </div>
            </article>
          ))}
        </div>
      ) : <p className="settings-empty-copy">导入凭证后，点击“授权频道”绑定上传账号。</p>}
    </section>
  );
}
