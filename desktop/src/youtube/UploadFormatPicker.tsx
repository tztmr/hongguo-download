import type { YouTubeUploadFormat } from "./types";
export function UploadFormatPicker({ value, onChange, disabled = false }: { value: YouTubeUploadFormat; onChange: (value: YouTubeUploadFormat) => void; disabled?: boolean }) {
  return <label className="auto-field"><span>上传类型</span><select aria-label="YouTube 上传类型" value={value} disabled={disabled} onChange={e => onChange(e.target.value as YouTubeUploadFormat)}><option value="auto">自动识别 · 保持原方式</option><option value="shorts">Shorts · 竖版 / 方形，不超过 3 分钟</option><option value="standard">普通 / 中长视频</option></select><small>{value === "shorts" ? "上传前检查真实画幅与时长；不自动裁剪或截断视频。Shorts 封面以 YouTube 实际展示为准。" : value === "standard" ? "短于或等于 3 分钟的竖版 / 方形会被 YouTube 识别为 Shorts，请先准备横版或更长成片。" : "YouTube 根据视频实际画幅与时长分类。"}</small></label>;
}
