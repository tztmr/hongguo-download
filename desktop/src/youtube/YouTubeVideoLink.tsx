import { useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";

export function YouTubeVideoLink({ url, label = "打开 YouTube 视频", title }: { url: string; label?: string; title?: string }) {
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState("");
  return <>
    <a href={url} title={title} target="_blank" rel="noreferrer" aria-disabled={opening} onClick={async (event) => {
      if (!isTauri()) return;
      // Prevent the WebView/plugin's delegated link handler from opening twice.
      event.preventDefault();
      if (opening) return;
      setOpening(true);
      setError("");
      try {
        await invoke("plugin:opener|open_url", { url });
      } catch (reason) {
        const detail = reason instanceof Error ? reason.message : typeof reason === "string" ? reason : "请检查默认浏览器设置";
        setError(`无法打开链接：${detail}`);
      } finally {
        setOpening(false);
      }
    }}>{opening ? "正在打开…" : label}</a>
    {error ? <small className="error-copy" role="alert">{error}</small> : null}
  </>;
}
