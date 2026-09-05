import { useEffect, useState } from "react";

export function Cover({ src, title, className = "" }: { src: string; title: string; className?: string }) {
  const [failed, setFailed] = useState(!src);
  useEffect(() => setFailed(!src), [src]);
  if (failed) {
    return <div className={`cover-fallback ${className}`} aria-label={`${title}封面占位`} />;
  }
  return <img className={className} src={src} alt={`${title}封面`} onError={() => setFailed(true)} />;
}
