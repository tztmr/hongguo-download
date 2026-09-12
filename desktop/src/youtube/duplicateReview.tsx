import type { YouTubeDuplicateMatch } from "./types";

export const duplicateReasonLabels: Record<YouTubeDuplicateMatch["reason"], string> = {
  sameDrama: "同剧、同季、同视频类型的上传记录",
  sameTitle: "标题相同，作品身份仍需核对",
  similarTitle: "剧名相近，不能确定为同一季",
  identityIncomplete: "季数或视频类型信息不完整",
};
export function hasConfirmedDuplicate(matches: YouTubeDuplicateMatch[]) {
  return matches.some(match => match.confidence === "confirmed");
}
export function validSeason(value: string) {
  return !value.trim() || (/^\d+$/.test(value) && Number(value) >= 1 && Number(value) <= 999);
}
export function seasonFields(value: string): { season?: number } {
  return value.trim() && validSeason(value) ? { season: Number(value) } : {};
}
export function DuplicateSeasonPicker({ value, onChange, disabled = false }: {
  value: string; onChange: (value: string) => void; disabled?: boolean;
}) {
  return <label className="auto-field"><span>查重季数（可选）</span>
    <input type="number" min="1" max="999" step="1" value={value} disabled={disabled}
      placeholder="留空：自动识别剧名中的季数" onChange={event => onChange(event.target.value)} />
    <small>{validSeason(value) ? "支持第二季、第2季、Season 2、S02；无季数不默认当作第一季。此设置仅用于查重，不改发布标题。" : "请输入 1～999 的整数，或留空自动识别。"}</small>
  </label>;
}
