import type { AIComponentStatus } from "../types";
import "./ai-install-progress.css";

const stages: Record<string, string> = {
  checking: "检查磁盘空间", downloading: "正在下载", verifying: "校验文件",
  extracting: "解压组件", selfTesting: "运行自检", installed: "安装完成", failed: "安装失败，可重试",
};

export function AIInstallProgress({ ids, components = [] }: { ids: string[]; components?: AIComponentStatus[] }) {
  return <div className="ai-install-progress-list" aria-label="AI 组件安装进度">
    {ids.map(id => {
      const item = components.find(component => component.id === id);
      const stage = item?.stage || (item?.installed ? "installed" : "waiting");
      const percent = stage === "installed" ? 100 : Math.round(Number.isFinite(item?.percent) ? Math.max(0, Math.min(100, item!.percent!)) : 0);
      const label = stages[stage] || "等待安装";
      return <div className={`ai-install-progress-item ${stage === "failed" ? "failed" : ""}`} key={id}>
        <div className="ai-install-progress-label"><strong>{id}</strong><span>{label} · {percent}%</span></div>
        <div className="progress-track" role="progressbar" aria-label={`${id} 安装进度`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent} aria-valuetext={`${label}，安装总进度 ${percent}%`}><span style={{ width: `${percent}%` }} /></div>
      </div>;
    })}
    <small>显示安装总进度，包含下载、校验、解压和自检；完成后自动继续处理。</small>
  </div>;
}
