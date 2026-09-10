import { YouTubeManagement } from "../youtube/YouTubeManagement";
import type { YouTubeModel } from "../youtube/types";

export function PlatformVideosPage({ youtube }: { youtube: YouTubeModel }) {
  const channel = youtube.channels.find((item) => item.channelId === youtube.activeChannelId);
  return <main className="platform-videos-page">
    <header className="platform-videos-header"><h1>平台视频管理</h1><p>YouTube · 标题、简介、封面、播放列表与可见性</p></header>
    <YouTubeManagement channelId={youtube.activeChannelId} channelTitle={channel?.title ?? ""} />
  </main>;
}
