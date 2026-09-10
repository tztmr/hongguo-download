import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Cover } from "../components/Cover";
import { YouTubeVideoLink } from "./YouTubeUploadJobs";
import { managementCommands, type ManagedPlaylist, type ManagedVideo, type ManagementCommands } from "./managementCommands";
import type { YouTubePrivacy } from "./types";

const privacyLabels = { private: "私人", unlisted: "不公开", public: "公开" };
function message(error: unknown) { return error && typeof error === "object" && "message" in error ? String(error.message) : typeof error === "string" ? error : "操作失败，请重试"; }
function PrivacyOptions() { return <>{Object.entries(privacyLabels).map(([id, label]) => <option key={id} value={id}>{label}</option>)}</>; }

export function YouTubeManagement({ channelId, channelTitle, commands = managementCommands }: { channelId: string | null; channelTitle: string; commands?: ManagementCommands }) {
  // Remount all channel-specific state, including any outstanding editor, on account changes.
  if (!channelId) return <div className="download-empty"><h3>尚未连接 YouTube 频道</h3><p>请先在设置中授权并选择 YouTube 频道，再管理已上传的视频。</p></div>;
  return <ChannelManagement key={channelId} channelId={channelId} channelTitle={channelTitle} commands={commands} />;
}
function ChannelManagement({ channelId, channelTitle, commands }: { channelId: string; channelTitle: string; commands: ManagementCommands }) {
  const [videos, setVideos] = useState<ManagedVideo[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [privacy, setPrivacy] = useState("all");
  const [editing, setEditing] = useState<ManagedVideo>();
  const generation = useRef(0);
  const loadingRef = useRef(false);
  async function load(more = false) {
    if (loadingRef.current) return;
    loadingRef.current = true;
    const request = ++generation.current;
    setLoading(true); setError("");
    try {
      const page = await commands.list(channelId, more ? next ?? undefined : undefined);
      if (request !== generation.current) return;
      setVideos((existing) => [...new Map([...(more ? existing : []), ...page.items].map((video) => [video.id, video])).values()]);
      setNext(page.nextPageToken);
    } catch (error) { if (request === generation.current) setError(message(error)); }
    finally { if (request === generation.current) { loadingRef.current = false; setLoading(false); } }
  }
  useEffect(() => { void load(); return () => { generation.current++; loadingRef.current = false; }; }, [channelId, commands]);
  const visible = videos.filter((video) => (privacy === "all" || video.privacyStatus === privacy) && `${video.title} ${video.id}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()));
  return <section className="youtube-management" aria-label="视频管理">
    <div className="yt-management-heading"><div><h2>频道视频</h2><p>{channelTitle || channelId} · 管理已上传到 YouTube 的视频</p></div><button type="button" className="secondary-button" disabled={loading || !!editing} onClick={() => void load()}>刷新频道视频</button></div>
    <div className="yt-management-toolbar"><input type="search" aria-label="搜索频道视频" placeholder="搜索已加载的视频标题或 ID" value={query} onChange={(e) => setQuery(e.target.value)} /><select aria-label="筛选视频可见性" value={privacy} onChange={(e) => setPrivacy(e.target.value)}><option value="all">全部可见性</option><PrivacyOptions /></select><span>已加载 {videos.length} 个 · 显示 {visible.length} 个{next ? " · 还有更多" : ""}</span></div>
    {error && <div className="warning-banner" role="alert">{error}</div>}
    <div className="yt-video-list">
      {visible.map((video) => <article className="yt-video-row" key={video.id}>
        <Cover src={video.thumbnailUrl} title={video.title} className="yt-video-cover" />
        <div className="yt-video-copy"><h3>{video.title}</h3><p>{video.description || "暂无简介"}</p><div className="yt-video-meta"><span className={`yt-privacy yt-privacy-${video.privacyStatus}`}>{privacyLabels[video.privacyStatus]}</span><span>{video.publishedAt ? new Date(video.publishedAt).toLocaleDateString() : ""}</span><YouTubeVideoLink url={`https://www.youtube.com/watch?v=${encodeURIComponent(video.id)}`} /></div></div>
        <button type="button" className="secondary-button" aria-label={`编辑 ${video.title}`} onClick={() => setEditing(video)}>编辑资料</button>
      </article>)}
      {!loading && !visible.length && <div className="download-empty"><h3>{error ? "未能读取频道视频" : videos.length || next ? "没有匹配的视频" : "频道暂无可管理的视频"}</h3><p>{videos.length || next ? "可以调整筛选条件，或继续加载更多视频。" : "点击刷新重新读取频道。"}</p></div>}
    </div>
    <div className="yt-management-footer">{loading ? <span role="status">正在读取频道视频…</span> : next ? <button type="button" className="secondary-button" onClick={() => void load(true)}>加载更多视频</button> : videos.length > 0 && !error ? <span>已加载全部频道视频</span> : null}</div>
    {editing && <VideoEditor key={editing.id} video={editing} channelId={channelId} commands={commands} onClose={() => setEditing(undefined)} onSaved={(video) => { setVideos((rows) => rows.map((row) => row.id === video.id ? video : row)); }} />}
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
