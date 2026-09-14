import { YouTubeVideoLink } from "../youtube/YouTubeVideoLink";

function validId(value?: string | null) {
  return value && /^[A-Za-z0-9_-]+$/.test(value) ? value : "";
}

export function youtubeVideoId(value?: string | null) {
  if (!value) return "";
  try {
    const url = new URL(value);
    if (url.protocol !== "https:" && url.protocol !== "http:") return "";
    const parts = url.pathname.split("/").filter(Boolean);
    if (url.hostname === "youtu.be") return validId(parts[0]);
    if (url.hostname !== "youtube.com" && !url.hostname.endsWith(".youtube.com")) return "";
    return validId(url.pathname === "/watch" ? url.searchParams.get("v") : ["shorts", "live"].includes(parts[0]) ? parts[1] : "");
  } catch { return ""; }
}

export function ShortsRelatedVideo({ shortVideoUrl, mainVideoUrl }: {
  shortVideoUrl?: string; mainVideoUrl?: string;
}) {
  const id = youtubeVideoId(shortVideoUrl);
  if (!id) return null;
  const mainId = youtubeVideoId(mainVideoUrl);
  return <section className="auto-related-video" aria-label="Shorts 关联正片">
    <YouTubeVideoLink url={`https://studio.youtube.com/video/${id}/edit`}
      label="相关视频 / Related video ↗"
      title="在 YouTube Studio 为此 Shorts 手动选择相关视频并保存" />
    <small>上传成功不会自动关联正片。请在 Studio 选择「相关视频」并保存；已手动设置的可前往核对。</small>
    {mainId && mainId !== id
      ? <p>对应正片：<YouTubeVideoLink url={mainVideoUrl!} label={mainVideoUrl} /></p>
      : <p>{mainId === id ? "正片与 Shorts 是同一条视频，请在 Studio 选择同频道的其他视频。" : "此任务未记录有效的正片地址，请在 Studio 核对同频道正片。"}</p>}
    <details><summary>如何设置 / 找不到此选项？</summary>
      <p>打开上方入口，切换到上传此 Shorts 的频道，在视频详情中选择「相关视频 / Related video」，然后保存。</p>
      <p>频道须开通高级功能，关联目标须是同频道的公开视频或不公开视频。若入口未显示，请先确认该视频在 Studio 中被归类为 Shorts，再到「设置 → 频道 → 功能使用资格」检查高级功能。</p>
      <p>Shorts 简介里的正片地址不会生成播放器中的相关视频链接。</p>
      <YouTubeVideoLink url="https://support.google.com/youtube/answer/14075157?hl=zh-Hans" label="YouTube 官方设置说明 ↗" />
    </details>
  </section>;
}
