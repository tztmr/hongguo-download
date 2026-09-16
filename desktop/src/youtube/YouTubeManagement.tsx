import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Cover } from "../components/Cover";
import { YouTubeVideoLink } from "./YouTubeUploadJobs";
import { managementCommands, type ManagedPlaylist, type ManagedVideo, type ManagementCommands } from "./managementCommands";
import type { YouTubePrivacy } from "./types";
import { notificationVideoIds } from "./managementVideoInput";
import { VideoBulkActionDialog, type VideoBulkAction } from "./VideoBulkActionDialog";

const privacyLabels = { private: "私人", unlisted: "不公开", public: "公开" };
function message(error: unknown) { return error && typeof error === "object" && "message" in error ? String(error.message) : typeof error === "string" ? error : "操作失败，请重试"; }
function PrivacyOptions() { return <>{Object.entries(privacyLabels).map(([id, label]) => <option key={id} value={id}>{label}</option>)}</>; }
function isBlocked(video: ManagedVideo) { return ["global", "region", "copyright"].includes(video.restriction.kind); }

export function YouTubeManagement({ channelId, channelTitle, commands = managementCommands }: { channelId: string | null; channelTitle: string; commands?: ManagementCommands }) {
  // Remount all channel-specific state, including any outstanding editor, on account changes.
  if (!channelId) return <div className="download-empty"><h3>尚未连接 YouTube 频道</h3><p>请先在设置中授权并选择 YouTube 频道，再管理已上传的视频。</p></div>;
  return <ChannelManagement key={channelId} channelId={channelId} channelTitle={channelTitle} commands={commands} />;
}
const formatLabels = { shorts: "Shorts（上传记录）", shortsCandidate: "Shorts（待确认）", standard: "普通视频", unknown: "类型待确认" };
function restrictionDetails(video: ManagedVideo) {
  const r = video.restriction;
  const regions = r.allowedRegions?.length ? `仅允许：${r.allowedRegions.join("、")}` : r.blockedRegions.length ? `封锁地区：${r.blockedRegions.join("、")}` : "";
  return [r.reason, regions].filter(Boolean).join(" · ");
}
function ChannelManagement({ channelId, channelTitle, commands }: { channelId: string; channelTitle: string; commands: ManagementCommands }) {
  const [videos, setVideos] = useState<ManagedVideo[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [scanning, setScanning] = useState(false);
  const [complete, setComplete] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [preparing, setPreparing] = useState<VideoBulkAction>();
  const [query, setQuery] = useState("");
  const [privacy, setPrivacy] = useState("all");
  const [format, setFormat] = useState("all");
  const [restriction, setRestriction] = useState("all");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [editing, setEditing] = useState<ManagedVideo>();
  const [bulk, setBulk] = useState<{ action: VideoBulkAction; videos: ManagedVideo[]; skippedPrivate?: number }>();
  const [notificationText, setNotificationText] = useState("");
  const [lookupIds, setLookupIds] = useState<Set<string>>();
  const [lookupFailures, setLookupFailures] = useState<{ videoId: string; message: string }[]>([]);
  const generation = useRef(0);
  const loadingRef = useRef(false);
  const stopScan = useRef(false);
  const pageTokens = useRef(new Set<string>());
  const allCheckbox = useRef<HTMLInputElement>(null);
  const locked = loading || !!editing || !!bulk;
  async function load(more = false, all = false, action?: VideoBulkAction) {
    if (loadingRef.current) return;
    loadingRef.current = true; stopScan.current = false;
    const request = ++generation.current;
    setLoading(true); setScanning(all); setPreparing(action); setError(""); setNotice("");
    if (!more) { setNext(null); setComplete(false); setSelected(new Set()); setLookupIds(undefined); setLookupFailures([]); pageTokens.current.clear(); }
    let token = more ? next ?? undefined : undefined;
    const gathered = new Map((more ? videos : []).map((video) => [video.id, video]));
    if (action) setVideos([]);
    try {
      do {
        const page = await commands.list(channelId, token);
        if (request !== generation.current) return;
        for (const video of page.items) gathered.set(video.id, video);
        setVideos([...gathered.values()]);
        if (page.nextPageToken && pageTokens.current.has(page.nextPageToken)) {
          setNext(null);
          throw new Error("YouTube 返回重复分页，已停止加载；当前列表尚未确认完整，请刷新重试");
        }
        setNext(page.nextPageToken);
        setComplete(!page.nextPageToken);
        if (page.nextPageToken) pageTokens.current.add(page.nextPageToken);
        token = page.nextPageToken ?? undefined;
      } while (all && token && !stopScan.current);
      if (action) {
        if (stopScan.current) { setNotice("已取消封锁视频扫描，未执行批量操作。"); return; }
        const blocked = [...gathered.values()].filter(isBlocked);
        const targets = action === "private" ? blocked.filter((video) => video.privacyStatus !== "private") : blocked;
        setQuery(""); setPrivacy("all"); setFormat("all"); setRestriction("blocked");
        if (targets.length) setBulk({ action, videos: targets, skippedPrivate: blocked.length - targets.length });
        else setNotice(blocked.length ? `全频道扫描完成，${blocked.length} 个封锁视频均已是私人，无需修改。` : "全频道扫描完成，未发现 API 标记的封锁／地区限制／版权拒绝视频。");
      }
    } catch (error) { if (request === generation.current) setError(action ? `封锁视频扫描未完成，未执行批量操作；请重试。${message(error)}` : message(error)); }
    finally { if (request === generation.current) { loadingRef.current = false; setLoading(false); setScanning(false); setPreparing(undefined); } }
  }
  async function lookup() {
    if (loadingRef.current) return;
    const ids = notificationVideoIds(notificationText);
    if (!ids.length || ids.length > 50) { setError("请粘贴通知中的视频链接或 ID，每次查询 1～50 个；可一行一个。"); return; }
    loadingRef.current = true;
    const request = ++generation.current;
    setLoading(true); setError(""); setNotice(""); setSelected(new Set());
    try {
      const result = await commands.lookup(channelId, ids);
      if (request !== generation.current) return;
      setVideos((existing) => [...new Map([...existing.filter((video) => !ids.includes(video.id)), ...result.items].map((video) => [video.id, video])).values()]);
      setLookupIds(new Set(result.items.map((video) => video.id)));
      setLookupFailures(result.failures);
      setQuery(""); setPrivacy("all"); setFormat("all"); setRestriction("all");
    } catch (error) { if (request === generation.current) setError(message(error)); }
    finally { if (request === generation.current) { loadingRef.current = false; setLoading(false); } }
  }
  useEffect(() => { void load(); return () => { generation.current++; loadingRef.current = false; stopScan.current = true; }; }, [channelId, commands]);
  const visible = videos.filter((video) => (!lookupIds || lookupIds.has(video.id))
    && (privacy === "all" || video.privacyStatus === privacy)
    && (format === "all" || (format === "shorts" ? ["shorts", "shortsCandidate"].includes(video.videoFormat) : video.videoFormat === format))
    && (restriction === "all" || (restriction === "blocked" ? isBlocked(video) : video.restriction.kind === restriction))
    && `${video.title} ${video.id}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()));
  const selectedVideos = visible.filter((video) => selected.has(video.id));
  const allSelected = visible.length > 0 && selectedVideos.length === visible.length;
  useEffect(() => { if (allCheckbox.current) allCheckbox.current.indeterminate = selectedVideos.length > 0 && !allSelected; }, [selectedVideos.length, allSelected]);
  function filter(change: () => void) { change(); setSelected(new Set()); }
  function updated(video: ManagedVideo) { setVideos((rows) => rows.map((row) => row.id === video.id ? video : row)); }
  return <section className="youtube-management" aria-label="视频管理">
    <div inert={!!editing || !!bulk}>
      <div className="yt-management-heading"><div><h2>频道视频</h2><p>{channelTitle || channelId} · 管理视频与 Shorts</p></div><button type="button" className="secondary-button" disabled={locked} onClick={() => void load()}>刷新频道视频</button></div>
      <div className="yt-blocked-actions">
        <div><strong>封锁视频处理</strong><p>自动扫描当前频道的全部视频与 Shorts，处理全球封锁、地区限制和版权拒绝的视频。</p><small>扫描完成后核对清单；设为私人会跳过已经私人的视频。</small></div>
        <div className="yt-blocked-buttons"><button type="button" className="secondary-button danger" disabled={locked} onClick={() => void load(false, true, "delete")}>一键删除封锁视频</button><button type="button" className="secondary-button" disabled={locked} onClick={() => void load(false, true, "private")}>一键设为私人</button></div>
        {preparing && <div className="yt-blocked-progress" role="status"><span>正在扫描全频道封锁视频，已读取 {videos.length} 个…</span><button type="button" className="secondary-button" onClick={() => { stopScan.current = true; }}>取消扫描</button></div>}
      </div>
      <div className="yt-management-toolbar">
        <input type="search" aria-label="搜索频道视频" placeholder="搜索已加载的视频标题或 ID" value={query} disabled={locked} onChange={(e) => filter(() => setQuery(e.target.value))} />
        <select aria-label="筛选视频类型" value={format} disabled={locked} onChange={(e) => filter(() => setFormat(e.target.value))}><option value="all">视频与 Shorts</option><option value="shorts">Shorts（含待确认）</option><option value="standard">普通视频</option><option value="unknown">类型待确认</option></select>
        <select aria-label="筛选视频限制" value={restriction} disabled={locked} onChange={(e) => filter(() => setRestriction(e.target.value))}><option value="all">全部限制状态</option><option value="blocked">封锁／地区限制／版权拒绝</option><option value="global">全球封锁</option><option value="region">地区限制</option><option value="copyright">版权拒绝</option><option value="unavailable">其他不可用</option><option value="noneReported">API 未返回封锁信息</option></select>
        <select aria-label="筛选视频可见性" value={privacy} disabled={locked} onChange={(e) => filter(() => setPrivacy(e.target.value))}><option value="all">全部可见性</option><PrivacyOptions /></select>
      </div>
      <div className="yt-management-help">
        <p>列表包含视频与 Shorts。仅按已加载内容筛选；要查询全频道，请加载全部视频。</p>
        <details><summary>查询通知中的视频</summary>
          <p>YouTube 通知中的视频可以粘贴链接或 ID 查询，支持 Shorts 和 Studio 链接。每次最多 50 个，一行一个。</p>
          <textarea aria-label="通知中的视频链接或 ID" value={notificationText} disabled={locked} onChange={(e) => setNotificationText(e.target.value)} placeholder="粘贴版权封锁通知中的视频链接…" />
          <button type="button" className="secondary-button" disabled={locked || !notificationText.trim()} onClick={() => void lookup()}>查询通知视频</button>
        </details>
        <small>版权通知详情以 YouTube Studio 为准；“API 未返回封锁信息”不代表没有版权限制。Shorts 按本机上传记录或短竖屏画幅标记，待确认项可到 Studio 核对。</small>
      </div>
      {lookupIds && <div className="yt-management-toolbar"><span>通知查询结果：{lookupIds.size} 个可管理视频</span><button type="button" className="secondary-button" disabled={locked} onClick={() => { setLookupIds(undefined); setLookupFailures([]); setSelected(new Set()); }}>返回频道列表</button></div>}
      {lookupFailures.length > 0 && <div className="warning-banner" role="alert">{lookupFailures.map((failure) => <p key={failure.videoId}>{failure.videoId}：{failure.message}</p>)}</div>}
      {error && <div className="warning-banner" role="alert">{error}</div>}
      {notice && <div className="yt-save-notice" role="status">{notice}</div>}
      <div className="yt-management-selection">
        <label><input ref={allCheckbox} type="checkbox" aria-label="全选当前筛选结果" checked={allSelected} disabled={locked || !visible.length} onChange={() => setSelected(new Set(allSelected ? [] : visible.map((video) => video.id)))} />全选当前结果</label>
        <span>已加载 {videos.length} 个 · 显示 {visible.length} 个 · 已选 {selectedVideos.length} 个{next ? " · 还有更多" : ""}</span>
        <button type="button" className="secondary-button danger" disabled={locked || !selectedVideos.length} onClick={() => setBulk({ action: "delete", videos: [...selectedVideos] })}>批量删除（{selectedVideos.length}）</button>
      </div>
      <div className="yt-video-list">
        {visible.map((video) => <article className="yt-video-row" key={video.id}>
          <input type="checkbox" aria-label={`选择 ${video.title}`} checked={selected.has(video.id)} disabled={locked} onChange={() => setSelected((current) => { const ids = new Set(current); if (ids.has(video.id)) ids.delete(video.id); else ids.add(video.id); return ids; })} />
          <Cover src={video.thumbnailUrl} title={video.title} className={`yt-video-cover${video.videoFormat.startsWith("shorts") ? " yt-shorts-cover" : ""}`} />
          <div className="yt-video-copy"><h3>{video.title}</h3><p>{video.description || "暂无简介"}</p>
            <div className="yt-video-meta"><span className={`yt-privacy yt-privacy-${video.privacyStatus}`}>{privacyLabels[video.privacyStatus]}</span><span>{formatLabels[video.videoFormat]}</span><span>{video.durationSeconds ? `${Math.floor(video.durationSeconds / 60)}:${String(Math.floor(video.durationSeconds % 60)).padStart(2, "0")}` : ""}</span><span>{video.publishedAt ? new Date(video.publishedAt).toLocaleDateString() : ""}</span></div>
            <p className={`yt-restriction yt-restriction-${video.restriction.kind}`} title={restrictionDetails(video)}>{restrictionDetails(video)}</p>
            <div className="yt-video-meta"><YouTubeVideoLink url={video.videoFormat === "shorts" ? `https://www.youtube.com/shorts/${encodeURIComponent(video.id)}` : `https://www.youtube.com/watch?v=${encodeURIComponent(video.id)}`} /><YouTubeVideoLink url={`https://studio.youtube.com/video/${encodeURIComponent(video.id)}/edit`} label="Studio 核对限制 ↗" /></div>
          </div>
          <button type="button" className="secondary-button" disabled={locked} aria-label={`编辑 ${video.title}`} onClick={() => setEditing(video)}>编辑资料</button>
        </article>)}
        {!loading && !visible.length && <div className="download-empty"><h3>{error ? "未能读取频道视频" : videos.length || next || lookupIds ? "没有匹配的视频" : "频道暂无可管理的视频"}</h3><p>{videos.length || next || lookupIds ? "可以调整筛选条件，或继续加载更多视频。" : "点击刷新重新读取频道。"}</p></div>}
      </div>
      <div className="yt-management-footer">
        {loading ? <><span role="status">{scanning ? `正在加载全频道视频与 Shorts，已读取 ${videos.length} 个…` : "正在读取频道视频…"}</span>{scanning && !preparing && <button type="button" className="secondary-button" onClick={() => { stopScan.current = true; }}>停止继续加载</button>}</>
          : next ? <><button type="button" className="secondary-button" disabled={locked} onClick={() => void load(true)}>加载更多视频</button><button type="button" className="secondary-button" disabled={locked} onClick={() => void load(true, true)}>加载全部视频与 Shorts</button></>
          : complete && videos.length > 0 && !error && !lookupIds ? <span>已加载全部频道视频与 Shorts</span> : null}
      </div>
    </div>
    {editing && <VideoEditor key={editing.id} video={editing} channelId={channelId} commands={commands} onClose={() => setEditing(undefined)} onSaved={updated} />}
    {bulk && <VideoBulkActionDialog {...bulk} channelId={channelId} channelTitle={channelTitle} commands={commands} onUpdated={updated} onDeleted={(id) => { setVideos((rows) => rows.filter((row) => row.id !== id)); setSelected((current) => { const ids = new Set(current); ids.delete(id); return ids; }); setLookupIds((current) => current ? new Set([...current].filter((value) => value !== id)) : undefined); }} onClose={() => setBulk(undefined)} />}
  </section>;
}

function VideoEditor({ video, channelId, commands, onClose, onSaved }: { video: ManagedVideo; channelId: string; commands: ManagementCommands; onClose(): void; onSaved(video: ManagedVideo): void }) {
  const [current, setCurrent] = useState(video);
  const [title, setTitle] = useState(video.title);
  const [description, setDescription] = useState(video.description);
  const [privacy, setPrivacy] = useState(video.privacyStatus);
  const [coverPath, setCoverPath] = useState("");
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const alive = useRef(true);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [playlists, setPlaylists] = useState<ManagedPlaylist[]>();
  const [playlistTitle, setPlaylistTitle] = useState("");
  const [playlistPrivacy, setPlaylistPrivacy] = useState<YouTubePrivacy>("private");
  const [confirmClose, setConfirmClose] = useState(false);
  const dirty = title !== current.title || description !== current.description || privacy !== current.privacyStatus || !!coverPath;
  const descriptionBytes = new TextEncoder().encode(description).length;
  const valid = !!title.trim() && [...title].length <= 100 && descriptionBytes <= 5000 && !/[<>]/.test(title + description);
  const dialogRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    alive.current = true;
    const previous = document.activeElement as HTMLElement | null;
    dialogRef.current?.focus();
    return () => { alive.current = false; previous?.focus(); };
  }, []);
  function close() { if (busyRef.current) return; if (dirty) setConfirmClose(true); else onClose(); }
  async function action(work: () => Promise<void>) {
    if (busyRef.current) return;
    busyRef.current = true; setBusy(true); setError(""); setNotice("");
    try { await work(); } catch (error) { if (alive.current) setError(message(error)); }
    finally { busyRef.current = false; if (alive.current) setBusy(false); }
  }
  async function save() {
    await action(async () => {
      const updated = await commands.update({ channelId, videoId: video.id, etag: current.etag, title: title.trim(), description, privacyStatus: privacy });
      if (!alive.current) return;
      setCurrent(updated); setTitle(updated.title); setDescription(updated.description); setPrivacy(updated.privacyStatus); onSaved(updated);
      setNotice(updated.privacyStatus === privacy ? "资料已同步到 YouTube" : `资料已保存；YouTube 实际可见性为${privacyLabels[updated.privacyStatus]}`);
    });
  }
  return <div className="dialog-backdrop"><div ref={dialogRef} tabIndex={-1} className="yt-edit-dialog" role="dialog" aria-modal="true" aria-labelledby="yt-editor-title" onKeyDown={(event) => {
    if (event.key === "Escape") { event.stopPropagation(); close(); }
    if (event.key === "Tab") {
      const controls = Array.from(event.currentTarget.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), a[href]'));
      const first = controls[0], last = controls[controls.length - 1];
      if (event.shiftKey && (document.activeElement === first || document.activeElement === event.currentTarget)) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }
  }}>
    <header><div><h2 id="yt-editor-title">编辑视频资料</h2><p>修改后直接同步到当前 YouTube 频道</p></div><button type="button" className="icon-button" aria-label="关闭视频编辑" disabled={busy} onClick={close}>×</button></header>
    <div className="yt-editor-body">
      {error && <><div className="warning-banner" role="alert">{error}</div><button type="button" className="secondary-button" disabled={busy} onClick={() => void action(async () => { const fresh = await commands.detail(channelId, video.id); if (alive.current) { setCurrent(fresh); setTitle(fresh.title); setDescription(fresh.description); setPrivacy(fresh.privacyStatus); onSaved(fresh); setNotice("已重新读取 YouTube 资料，请检查后编辑"); } })}>重新读取资料（替换当前输入）</button></>}{notice && <div className="yt-save-notice" role="status">{notice}</div>}
      {confirmClose && <div className="warning-banner">有未保存的修改。<button type="button" className="secondary-button" onClick={() => setConfirmClose(false)}>继续编辑</button><button type="button" className="secondary-button danger" onClick={onClose}>放弃修改并关闭</button></div>}
      <fieldset disabled={busy} className="yt-editor-fields"><legend>标题与简介</legend>
        <label>标题<input aria-label="视频标题" value={title} maxLength={100} onChange={(e) => setTitle(e.target.value)} /><small>{[...title].length} / 100</small></label>
        <label>内容简介<textarea aria-label="视频简介" value={description} onChange={(e) => setDescription(e.target.value)} /><small className={descriptionBytes > 5000 ? "error-copy" : ""}>{descriptionBytes} / 5000 字节</small></label>
        <label>可见性<select aria-label="视频可见性" value={privacy} onChange={(e) => setPrivacy(e.target.value as YouTubePrivacy)}><PrivacyOptions /></select></label>
        <small>私人：仅指定的人可看；不公开：知道链接的人可看；公开：所有人可看。</small>
        <button type="button" className="primary-button compact" disabled={!valid || busy} onClick={() => void save()}>保存资料到 YouTube</button>
      </fieldset>
      <fieldset disabled={busy} className="yt-editor-fields"><legend>视频封面</legend><div className="yt-cover-edit"><Cover src={current.thumbnailUrl} title={current.title} className="yt-video-cover" /><div><p>{coverPath || "选择图片，更换频道视频的封面"}</p><div className="yt-inline-actions"><button type="button" className="secondary-button" onClick={() => void action(async () => { const path = await open({ multiple: false, directory: false, filters: [{ name: "封面图片", extensions: ["jpg", "jpeg", "png", "webp"] }] }); if (alive.current && typeof path === "string") setCoverPath(path); })}>选择本地封面</button><button type="button" className="secondary-button" disabled={!coverPath || busy} onClick={() => void action(async () => { await commands.thumbnail(channelId, video.id, coverPath); if (alive.current) { setCoverPath(""); setNotice("封面已同步到 YouTube，预览可能需要稍后刷新"); } const fresh = await commands.detail(channelId, video.id); if (alive.current) { if (fresh.title === current.title && fresh.description === current.description && fresh.privacyStatus === current.privacyStatus) { setCurrent((old) => ({ ...old, etag: fresh.etag, thumbnailUrl: fresh.thumbnailUrl })); } else { setError("封面已上传，但视频资料已被其他操作修改，请重新读取资料后再保存"); } onSaved(fresh); } })}>上传新封面</button></div></div></div></fieldset>
      <fieldset disabled={busy} className="yt-editor-fields"><legend>播放列表（清单）</legend><p>将本视频加入或移出频道的播放列表，每次操作立即同步。</p><button type="button" className="secondary-button" onClick={() => void action(async () => { const rows = await commands.playlists(channelId, video.id); if (alive.current) setPlaylists(rows); })}>{playlists ? "刷新播放列表" : "读取播放列表"}</button>
        {playlists?.map((playlist) => <div className="yt-playlist-row" key={playlist.id}><div><strong>{playlist.title}</strong><small>{privacyLabels[playlist.privacyStatus]} · {playlist.itemIds.length ? "已加入" : "未加入"}</small></div><button type="button" className="secondary-button" aria-label={`${playlist.itemIds.length ? "移出" : "加入"} ${playlist.title}`} onClick={() => void action(async () => { await commands.membership(channelId, video.id, playlist.id, !playlist.itemIds.length); const rows = await commands.playlists(channelId, video.id); if (alive.current) { setPlaylists(rows); setNotice("播放列表已同步到 YouTube"); } })}>{playlist.itemIds.length ? "移出清单" : "加入清单"}</button></div>)}
        {playlists?.length === 0 && <small>频道暂无播放列表，可以在下面新建。</small>}
        <div className="yt-new-playlist"><input aria-label="新播放列表名称" placeholder="新播放列表名称" value={playlistTitle} maxLength={150} onChange={(e) => setPlaylistTitle(e.target.value)} /><select aria-label="新播放列表可见性" value={playlistPrivacy} onChange={(e) => setPlaylistPrivacy(e.target.value as YouTubePrivacy)}><PrivacyOptions /></select><button type="button" className="secondary-button" disabled={!playlistTitle.trim() || busy} onClick={() => void action(async () => { await commands.createPlaylist(channelId, playlistTitle.trim(), playlistPrivacy); if (alive.current) { setPlaylistTitle(""); setNotice("播放列表已创建，可读取列表后将视频加入"); } const rows = await commands.playlists(channelId, video.id); if (alive.current) setPlaylists(rows); })}>新建清单</button></div>
      </fieldset>
    </div><footer>{busy ? <span role="status">正在同步，请稍候…</span> : <YouTubeVideoLink url={`https://www.youtube.com/watch?v=${encodeURIComponent(video.id)}`} />}<button type="button" className="secondary-button" disabled={busy} onClick={close}>完成</button></footer>
  </div></div>;
}
