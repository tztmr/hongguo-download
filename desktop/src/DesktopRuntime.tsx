import { type ReactNode, useEffect, useState } from "react";

export function hasDesktopBridge() {
  return typeof (window as Window & { __TAURI_INTERNALS__?: { invoke?: unknown } }).__TAURI_INTERNALS__?.invoke === "function";
}
export function desktopHost() {
  return window.location.protocol === "tauri:" || ["tauri.localhost", "tauri.localhost.localdomain"].includes(window.location.hostname)
    || !!(window as Window & { isTauri?: boolean }).isTauri;
}
export function DesktopRuntime({ children }: { children: ReactNode }) {
  const preview = ["library", "downloads", "automation"].includes(new URLSearchParams(window.location.search).get("preview") || "");
  const [ready, setReady] = useState(hasDesktopBridge);
  const [waiting, setWaiting] = useState(!ready && desktopHost());
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (preview || ready) return;
    const probe = () => { if (hasDesktopBridge()) { setReady(true); setWaiting(false); } };
    probe();
    const poll = window.setInterval(probe, 100);
    const timeout = window.setTimeout(() => { window.clearInterval(poll); setWaiting(false); }, 5000);
    return () => { window.clearInterval(poll); window.clearTimeout(timeout); };
  }, [preview, ready, attempt]);
  if (preview) return <div className="runtime-preview"><div className="runtime-preview-notice" role="status">界面演示 · 剧目与频道均为示例数据，下载和 YouTube 修改不会实际执行。</div><div className="runtime-preview-content">{children}</div></div>;
  if (ready) return children;
  return <main className="runtime-unavailable"><section>
    <span className="runtime-brand">红果下载</span>
    <h1>{waiting ? "正在连接桌面功能…" : desktopHost() ? "桌面功能尚未就绪" : "请从桌面应用打开红果下载"}</h1>
    <p>{desktopHost() ? "正在等待应用初始化。可重试连接；若持续未就绪，请退出并重新打开红果下载。" : "当前页面在浏览器中打开，无法连接本地下载、AI 处理和账号管理功能。请启动已安装的红果下载应用。"}</p>
    <div><button type="button" className="primary-button compact" onClick={() => { setWaiting(desktopHost()); setAttempt(value => value + 1); }}>重新检测连接</button><a className="secondary-button" href="?preview=library">进入界面演示</a></div>
    <small>界面演示仅用于查看布局，不会读取真实剧目或修改 YouTube 视频。</small>
  </section></main>;
}
