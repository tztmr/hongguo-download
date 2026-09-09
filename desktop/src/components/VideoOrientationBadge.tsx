import { useEffect, useRef, useState } from "react";
import { detectVideoOrientation, type VideoOrientation } from "../videoOrientation";

export function VideoOrientationBadge({ seriesId, firstVid, enabled = true }: {
  seriesId: string;
  firstVid: string;
  enabled?: boolean;
}) {
  const badgeRef = useRef<HTMLSpanElement>(null);
  const [result, setResult] = useState<{ key: string; orientation: VideoOrientation } | null>(null);
  const key = JSON.stringify([seriesId, firstVid]);
  const orientation = result?.key === key ? result.orientation : null;

  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let started = false;
    const detect = () => {
      if (started || disposed) return;
      started = true;
      void detectVideoOrientation({ seriesId, firstVid }).then((value) => {
        if (!disposed) setResult({ key, orientation: value });
      });
    };
    const observer = typeof IntersectionObserver === "undefined" ? null : new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        observer?.disconnect();
        detect();
      }
    });
    if (observer && badgeRef.current) observer.observe(badgeRef.current);
    else detect();
    return () => {
      disposed = true;
      observer?.disconnect();
    };
  }, [seriesId, firstVid, enabled, key]);

  if (!enabled) return null;
  const label = orientation === "未知" ? "方向未知" : orientation || "检测中";
  return <span ref={badgeRef} className={`video-orientation-badge${orientation && orientation !== "未知" ? " detected" : ""}`} title={orientation === "未知" ? "暂未获取到有效的视频宽高" : "根据该剧视频源的宽高识别横竖屏"}>{label}</span>;
}
