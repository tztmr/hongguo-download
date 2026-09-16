import { memo } from "react";
import { YouTubeManagement } from "../youtube/YouTubeManagement";
import type { ManagementCommands } from "../youtube/managementCommands";
import type { YouTubeModel } from "../youtube/types";

export const PlatformVideosPage = memo(function PlatformVideosPage({ youtube, commands, hidden = false }: { hidden?: boolean; youtube: YouTubeModel; commands?: ManagementCommands }) {
  const channel = youtube.channels.find((item) => item.channelId === youtube.activeChannelId);
  return <main hidden={hidden} className="platform-videos-page">
    <header className="platform-videos-header"><h1>视频管理</h1><p>YouTube · 视频与 Shorts · 封锁查询、批量删除与设为私人</p></header>
    <YouTubeManagement channelId={youtube.activeChannelId} channelTitle={channel?.title ?? ""} commands={commands} />
  </main>;
}, (a, b) => a.hidden === b.hidden && a.commands === b.commands
  && a.youtube.activeChannelId === b.youtube.activeChannelId && a.youtube.channels === b.youtube.channels);
