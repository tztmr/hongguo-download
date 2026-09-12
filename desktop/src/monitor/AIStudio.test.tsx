import { useState } from "react";
import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AutomationPage } from "./AutomationPage";
import { AIStudio, studioDefaults, type AIStudioSettings } from "./AIStudio";
import { StudioRequestError, studioRequest } from "./studioApi";

vi.mock("./studioApi", async importOriginal => ({ ...await importOriginal<typeof import("./studioApi")>(), studioRequest: vi.fn() }));
const defaults: AIStudioSettings = { ...studioDefaults, imageSize: "1024x1024", metadataSource: "deepseek", textModel: "deepseek-v4-pro", coverSource: "moyuu", coverModel: "gpt-image-2", textPrompt: "准确", coverPrompt: "参考源图" };
const result = { title_candidates: [{ title: "真实返回的标题", angle: "冲突", evidence: "源简介" }], recommended_title: "真实返回的标题", description: "真实描述", tags: ["短剧"], category_suggestion: { name: "娱乐" }, cover_concepts: [{ headline: "真相", prompt: "ledger in boardroom" }] };
function Harness({ onFallback }: { onFallback?: () => void } = {}) { const [settings, setSettings] = useState(defaults); return <AIStudio settings={settings} onFallback={onFallback} onChange={(key, value) => setSettings(s => ({ ...s, [key]: value }))} />; }
afterEach(() => { cleanup(); vi.unstubAllGlobals(); vi.mocked(studioRequest).mockReset(); });
function fill(view: ReturnType<typeof render>) {
  fireEvent.change(view.getByLabelText("文字服务 API Key", { exact: false }), { target: { value: "text-fixture-key" } });
  fireEvent.change(view.getByLabelText("封面服务 API Key", { exact: false }), { target: { value: "image-fixture-key" } });
  fireEvent.change(view.getByLabelText("原标题 / 剧名"), { target: { value: "原剧名" } });
  fireEvent.change(view.getByLabelText(/^视频内容 \/ 字幕/), { target: { value: "真实剧情内容" } });
}
describe("AI studio live requests", () => {
  it("sends fixed A/B/C rules for existing settings and applies hashtags to the upload description", async () => {
    const onApply = vi.fn();
    const generated = { ...result, title_candidates: [
      { title: "标题甲", angle: "情绪冲突型" }, { title: "标题乙", angle: "剧情反转型" }, { title: "标题丙", angle: "强点击型" },
    ], recommended_title: "标题乙", recommendation_reason: "反转有剧情依据", tags: ["原剧名", "短剧"] };
    vi.mocked(studioRequest).mockResolvedValue({ result: generated });
    const view = render(<AIStudio settings={{...defaults, textPrompt: "旧草稿附加要求"}} onChange={vi.fn()} onApply={onApply} />); fill(view);
    fireEvent.click(view.getByRole("button", {name: "生成真实文案"}));
    await waitFor(() => expect(view.getByLabelText("AI 推荐标题").textContent).toContain("最推荐：标题B"));
    const prompt = String(vi.mocked(studioRequest).mock.calls[0][1].prompt);
    for (const rule of ["只生成 3 个标题", "35～60", "300～500", "8～12", "不剧透最终结局", "季数", "旧草稿附加要求", "原剧名", "真实剧情内容"]) expect(prompt).toContain(rule);
    expect(prompt).not.toContain("输出 5 个候选标题");
    expect(view.getByLabelText("生成的 Hashtag").textContent).toContain("#原剧名 #短剧");
    fireEvent.click(view.getByRole("button", {name: /C\s*标题丙/}));
    fireEvent.click(view.getByRole("button", {name: "应用文案到上传草稿"}));
    expect(onApply).toHaveBeenCalledWith("标题丙", "真实描述\n\n#原剧名 #短剧", ["原剧名", "短剧"]);
  });
  it("uses distinct keys and retains the image prompt when image mode changes", async () => {
    vi.mocked(studioRequest).mockResolvedValueOnce({ result }).mockResolvedValueOnce({ image: "https://example.com/cover.png", model: "gpt-image-2", usedReference: false });
    const view = render(<Harness />); fill(view);
    fireEvent.click(view.getByRole("button", { name: "生成真实文案" }));
    await waitFor(() => expect(view.getByRole("textbox", { name: "选用标题" })).toBeTruthy());
    expect(vi.mocked(studioRequest).mock.calls[0][1].apiKey).toBe("text-fixture-key");
    fireEvent.change(view.getByLabelText("封面生成模式"), { target: { value: "text" } });
    expect((view.getByLabelText("本次封面补充要求（可选）") as HTMLTextAreaElement).value).toContain("ledger");
    fireEvent.click(view.getByRole("button", { name: "生成真实封面" }));
    await waitFor(() => expect(view.getByRole("img", { name: "AI 接口实际生成的封面" })).toBeTruthy());
    expect(vi.mocked(studioRequest).mock.calls[1][1].apiKey).toBe("image-fixture-key");
    expect(vi.mocked(studioRequest).mock.calls[1][1].referenceImage).toBeUndefined();
  });
  it("sends the fixed template and current source image without first generating text, overriding old square sizes", async () => {
    vi.stubGlobal("URL", Object.assign(class extends URL {}, { createObjectURL: vi.fn(() => "blob:source"), revokeObjectURL: vi.fn() }));
    vi.mocked(studioRequest).mockResolvedValue({ image: "https://example.com/cover.png", model: "gpt-image-2", usedReference: true });
    const view = render(<Harness />); fill(view);
    fireEvent.change(view.getByLabelText("原标题 / 剧名"), { target: { value: "贺太太不做替身了" } });
    fireEvent.change(view.getByLabelText("选择源封面"), { target: { files: [new File(["image-fixture"], "poster.png", { type: "image/png" })] } });
    fireEvent.change(view.getByLabelText("本次封面补充要求（可选）"), { target: { value: "蓝金背景" } });
    const preview = view.getByLabelText("图片 AI 固定提示词") as HTMLTextAreaElement;
    expect(preview.readOnly).toBe(true);
    fireEvent.click(view.getByRole("button", { name: "生成真实封面" }));
    await waitFor(() => expect(studioRequest).toHaveBeenCalledTimes(1));
    const [action, payload] = vi.mocked(studioRequest).mock.calls[0];
    expect(action).toBe("image");
    expect(payload.size).toBe("1536x864");
    expect(payload.referenceImage).toMatch(/^data:image\/png;base64,/);
    expect(payload.prompt).toBe(preview.value);
    for (const rule of ["贺太太不做替身了", "真实剧情内容", "蓝金背景", "红底黄字", "金色数字", "8～12", "不要集数", "50%～65%", "2～4"]) expect(payload.prompt).toContain(rule);
    fireEvent.change(view.getByLabelText("原标题 / 剧名"), { target: { value: "新作品" } });
    expect(preview.value).toContain("新作品");
    expect(preview.value).not.toContain("贺太太不做替身了");
  });
  it("does not show stale text after the user changes source material", async () => {
    let finish!: (value: unknown) => void;
    vi.mocked(studioRequest).mockReturnValue(new Promise(resolve => { finish = resolve; }));
    const view = render(<Harness />); fill(view);
    fireEvent.click(view.getByRole("button", { name: "生成真实文案" }));
    fireEvent.change(view.getByLabelText("原标题 / 剧名"), { target: { value: "另一部剧" } });
    await act(async () => finish({ result }));
    expect(view.queryByRole("textbox", { name: "选用标题" })).toBeNull();
  });
  it("clears image credentials when changing providers and never calls image generation without a reference", async () => {
    const view = render(<Harness />); fill(view);
    fireEvent.change(view.getByLabelText("本次封面补充要求（可选）"), { target: { value: "生成封面" } });
    fireEvent.click(view.getByRole("button", { name: "生成真实封面" }));
    await waitFor(() => expect(view.getByRole("status").textContent).toContain("参考图模式需要"));
    expect(studioRequest).not.toHaveBeenCalled();
    fireEvent.change(view.getByLabelText("封面服务", { exact: true }), { target: { value: "jucodex" } });
    expect((view.getByLabelText("封面服务 API Key", { exact: false }) as HTMLInputElement).value).toBe("");
    expect((view.getByLabelText("文字服务 API Key", { exact: false }) as HTMLInputElement).value).toBe("text-fixture-key");
  });
  it.each(["生成真实文案", "生成真实封面"])("skips missing credentials without an API request: %s", async button => {
    const fallback = vi.fn();
    const view = render(<Harness onFallback={fallback} />);
    fireEvent.click(view.getByRole("button", { name: button }));
    await waitFor(() => expect(fallback).toHaveBeenCalledTimes(1));
    expect(studioRequest).not.toHaveBeenCalled();
    expect(view.getByLabelText("非 AI 上传回退")).toBeTruthy();
  });
  it.each(["生成真实文案", "生成真实封面"])("falls back for typed credential errors without retrying: %s", async button => {
    const fallback = vi.fn();
    vi.mocked(studioRequest).mockRejectedValue(new StudioRequestError("AI_KEY_INVALID", "Key 已失效"));
    const view = render(<Harness onFallback={fallback} />); fill(view);
    fireEvent.change(view.getByLabelText("封面生成模式"), { target: { value: "text" } });
    fireEvent.click(view.getByRole("button", { name: button }));
    await waitFor(() => expect(fallback).toHaveBeenCalledTimes(1));
    expect(studioRequest).toHaveBeenCalledTimes(1);
    expect(view.getByRole("status").textContent).toContain("已跳过 AI");
    expect(view.queryByRole("textbox", { name: "选用标题" })).toBeNull();
  });
  it("sends V4 Flash under its exact API model name", async () => {
    vi.mocked(studioRequest).mockResolvedValue({ result });
    const view = render(<Harness />); fill(view);
    fireEvent.change(view.getByLabelText("DeepSeek 文字模型", { exact: false }), { target: { value: "deepseek-v4-flash" } });
    fireEvent.click(view.getByRole("button", { name: "生成真实文案" }));
    await waitFor(() => expect(studioRequest).toHaveBeenCalledWith("text", expect.objectContaining({ model: "deepseek-v4-flash" })));
  });
  it("renders a real service failure without substituting demo results", async () => {
    vi.mocked(studioRequest).mockRejectedValue(new Error("网络超时"));
    const view = render(<Harness />); fill(view);
    fireEvent.click(view.getByRole("button", { name: "生成真实文案" }));
    await waitFor(() => expect(view.getByRole("status").textContent).toBe("网络超时"));
    expect(view.queryByRole("textbox", { name: "选用标题" })).toBeNull();
  });
});

it("migrates saved Flash and restores custom non-AI templates when credentials are removed after applying AI", async () => {
  const saved = new Map<string, string>([["hongguo.automation.settings-draft.v1", JSON.stringify({metadataVersion: 3, textModel: "deepseek-flash", title: "我的频道 {剧名}", description: "原简介 {简介}", tags: "原标签", privacy: "unlisted"})]]);
  vi.stubGlobal("localStorage", {getItem: (key: string) => saved.get(key) ?? null, setItem: (key: string, value: string) => saved.set(key, value)});
  vi.mocked(studioRequest).mockResolvedValue({result});
  const view = render(<AutomationPage saveDir="/Downloads" />);
  fireEvent.click(view.getByRole("button", {name: /AI 文案与封面/}));
  expect((view.getByLabelText("DeepSeek 文字模型", {exact:false}) as HTMLSelectElement).value).toBe("deepseek-v4-flash");
  fill(view);
  fireEvent.click(view.getByRole("button", {name: "生成真实文案"}));
  await waitFor(() => expect(view.getByRole("button", {name: "应用文案到上传草稿"})).toBeTruthy());
  fireEvent.click(view.getByRole("button", {name: "应用文案到上传草稿"}));
  fireEvent.change(view.getByLabelText("文字服务 API Key", {exact:false}), {target: {value: ""}});
  fireEvent.click(view.getByRole("button", {name: /YouTube 上传/}));
  expect((view.getByLabelText("YouTube 标题") as HTMLInputElement).value).toBe("我的频道 {剧名}");
  expect((view.getByLabelText("YouTube 视频描述") as HTMLTextAreaElement).value).toBe("原简介 {简介}");
  expect((view.getByLabelText("视频标签", {exact:false}) as HTMLInputElement).value).toBe("原标签");
  expect((view.getByLabelText("上传可见性") as HTMLSelectElement).value).toBe("unlisted");
});
