import { useEffect, useRef } from "react";
import type { MediaJobsModel } from "../media/types";
import type { YouTubeModel } from "../youtube/types";
import type { NotificationAdapter, NotificationTarget } from "../notifications";

type Options = {
  adapter: NotificationAdapter;
  media: MediaJobsModel;
  youtube: YouTubeModel;
  notifyMedia: boolean;
  notifyYouTube: boolean;
  enabled?: boolean;
  onTarget(target: NotificationTarget): void;
};

export function useNotificationRouter({ adapter, media, youtube, notifyMedia, notifyYouTube, enabled = true, onTarget }: Options) {
  const pending = useRef(new Set<string>());
  const targetRef = useRef(onTarget);
  targetRef.current = onTarget;

  useEffect(() => {
    if (!enabled || !adapter.subscribeActions) return;
    let unlisten: (() => void) | undefined;
    void adapter.subscribeActions((target) => targetRef.current(target))
      .then((stop) => { unlisten = stop; })
      .catch(() => undefined);
    return () => unlisten?.();
  }, [adapter, enabled]);

  useEffect(() => {
    if (!enabled) return;
    for (const job of media.jobs) {
      const success = job.status === "completed";
      const failure = job.status === "failed" || job.status === "interrupted";
      if ((!success && !failure) || (success ? job.completionNotifiedAt : job.failureNotifiedAt)) continue;
      const key = `media:${job.id}:${success ? "success" : "failure"}`;
      if (pending.current.has(key)) continue;
      pending.current.add(key);
      const kind = job.kind === "merge" ? "视频合并" : job.kind === "separateBackgroundMusic" ? "背景音乐分离" : "字幕提取";
      const count = job.outputs?.length ? `，生成 ${job.outputs.length} 个结果` : "";
      const notification = {
        title: `${kind}${success ? "完成" : "失败"}`,
        body: `${success ? "任务已完成" : "任务未完成"}${count}`,
        target: { kind: "mediaJob" as const, id: job.id },
      };
      const attempt = notifyMedia ? adapter.send(notification) : Promise.resolve(false);
      void attempt.finally(async () => {
        try { await media.markNotified?.(job.id, success ? "success" : "failure"); } finally { pending.current.delete(key); }
      });
    }
  }, [adapter, enabled, media, media.jobs, notifyMedia]);

  useEffect(() => {
    if (!enabled) return;
    for (const job of youtube.jobs) {
      const success = job.status === "completed";
      const failure = job.status === "failed" || job.status === "videoUploadedThumbnailFailed" || job.status === "videoUploadedSubtitleFailed";
      if ((!success && !failure) || (success ? job.completionNotifiedAt : job.failureNotifiedAt)) continue;
      const key = `youtube:${job.id}:${success ? "success" : "failure"}`;
      if (pending.current.has(key)) continue;
      pending.current.add(key);
      const subtitleFailed = job.status === "videoUploadedSubtitleFailed";
      const partial = job.status === "videoUploadedThumbnailFailed" || subtitleFailed;
      const notification = {
        title: partial ? (subtitleFailed ? "视频已上传，字幕失败" : "视频已上传，封面失败") : `YouTube 上传${success ? "完成" : "失败"}`,
        body: `《${job.title.slice(0, 100)}》${partial ? (subtitleFailed ? "可在上传任务中仅重试字幕" : "可在上传任务中仅重试封面") : success ? "已完成" : "未完成"}`,
        target: { kind: "youtubeJob" as const, id: job.id },
      };
      const attempt = notifyYouTube ? adapter.send(notification) : Promise.resolve(false);
      void attempt.finally(async () => {
        try { await youtube.markNotified?.(job.id, success ? "success" : "failure"); } finally { pending.current.delete(key); }
      });
    }
  }, [adapter, enabled, notifyYouTube, youtube, youtube.jobs]);
}
