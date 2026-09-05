import type { NavId } from "../types";
import { BellIcon, ChartIcon, DownloadIcon, HomeIcon, SearchIcon, SettingsIcon } from "./icons";

type AppRailProps = {
  nav: NavId;
  pendingCount: number;
  unseenReleases: number;
  healthOk: boolean;
  onNavigate: (nav: NavId) => void;
};

const entries: Array<{ id: NavId; label: string; icon: typeof HomeIcon }> = [
  { id: "discover", label: "首页", icon: HomeIcon },
  { id: "search", label: "搜索", icon: SearchIcon },
  { id: "rank", label: "榜单", icon: ChartIcon },
  { id: "monitor", label: "新剧监听", icon: BellIcon },
  { id: "queue", label: "下载管理", icon: DownloadIcon },
  { id: "settings", label: "设置", icon: SettingsIcon },
];

export function AppRail({ nav, pendingCount, unseenReleases, healthOk, onNavigate }: AppRailProps) {
  return (
    <aside className="app-rail">
      <div className="app-mark" title="红果下载"><DownloadIcon size={19} /></div>
      <nav className="primary-nav" aria-label="主导航">
        {entries.map((entry) => {
          const EntryIcon = entry.icon;
          return (
            <button
              type="button"
              key={entry.id}
              className={`nav-item ${nav === entry.id ? "active" : ""}`}
              aria-current={nav === entry.id ? "page" : undefined}
              onClick={() => onNavigate(entry.id)}
            >
              <span className="nav-icon"><EntryIcon /></span>
              <span>{entry.label}</span>
              {entry.id === "queue" && pendingCount ? <b className="nav-badge">{pendingCount > 99 ? "99+" : pendingCount}</b> : null}
              {entry.id === "monitor" && unseenReleases ? <b className="nav-badge">{unseenReleases > 99 ? "99+" : unseenReleases}</b> : null}
            </button>
          );
        })}
      </nav>
      <div className="service-health"><span className={healthOk ? "online" : ""} />{healthOk ? "服务正常" : "服务离线"}</div>
    </aside>
  );
}
