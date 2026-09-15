import type { NavId } from "../types";
import { AutomationIcon, BellIcon, ChartIcon, DownloadIcon, HomeIcon, SearchIcon, SettingsIcon, PlayIcon } from "./icons";
import packageJson from "../../package.json";

type AppRailProps = {
  nav: NavId;
  pendingCount: number;
  unseenReleases: number;
  healthOk: boolean;
  onNavigate: (nav: NavId) => void;
};

const groups: Array<{ label: string; entries: Array<{ id: NavId; label: string; icon: typeof HomeIcon }> }> = [
  { label: "找剧", entries: [
    { id: "discover", label: "首页", icon: HomeIcon },
    { id: "search", label: "搜索", icon: SearchIcon },
    { id: "rank", label: "榜单", icon: ChartIcon },
  ] },
  { label: "自动处理", entries: [
    { id: "monitor", label: "新剧监听", icon: BellIcon },
    { id: "automation", label: "自动追剧", icon: AutomationIcon },
  ] },
  { label: "任务与发布", entries: [
    { id: "queue", label: "下载管理", icon: DownloadIcon },
    { id: "platformVideos", label: "视频管理", icon: PlayIcon },
    { id: "analytics", label: "数据分析", icon: ChartIcon },
  ] },
];

export function AppRail({ nav, pendingCount, unseenReleases, healthOk, onNavigate }: AppRailProps) {
  return (
    <aside className="app-rail">
      <div className="app-mark" title="红果下载"><DownloadIcon size={19} /></div>
      <nav className="primary-nav" aria-label="主导航">
        <div className="nav-scroll">{groups.map(group => <div role="group" aria-label={group.label} className="nav-group" key={group.label}>
          <span className="nav-group-label" aria-hidden="true">{group.label}</span>
        {group.entries.map((entry) => {
          const EntryIcon = entry.icon;
          return (
            <button
              type="button"
              key={entry.id}
              className={`nav-item ${nav === entry.id ? "active" : ""}`}
              aria-current={nav === entry.id ? "page" : undefined}
              title={`${group.label} · ${entry.label}`}
              onClick={() => onNavigate(entry.id)}
            >
              <span className="nav-icon"><EntryIcon /></span>
              <span>{entry.label}</span>
              {entry.id === "queue" && pendingCount ? <b className="nav-badge">{pendingCount > 99 ? "99+" : pendingCount}</b> : null}
              {entry.id === "monitor" && unseenReleases ? <b className="nav-badge">{unseenReleases > 99 ? "99+" : unseenReleases}</b> : null}
            </button>
          );
        })}</div>)}</div>
        <button type="button" className={`nav-item nav-settings ${nav === "settings" ? "active" : ""}`} aria-current={nav === "settings" ? "page" : undefined} onClick={() => onNavigate("settings")}>
          <span className="nav-icon"><SettingsIcon /></span><span>设置</span>
        </button>
      </nav>
      <div className="service-health"><span className={healthOk ? "online" : ""} />{healthOk ? "服务正常" : "服务离线"}<small>v{packageJson.version}</small></div>
    </aside>
  );
}
