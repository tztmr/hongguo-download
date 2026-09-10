import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AppRail } from "./AppRail";
import packageJson from "../../package.json";

describe("AppRail", () => {
  it("shows the pending download badge and changes the active workspace", () => {
    const onNavigate = vi.fn();
    const view = render(<AppRail nav="discover" pendingCount={21} unseenReleases={3} healthOk onNavigate={onNavigate} />);

    expect(view.getByText("21")).toBeTruthy();
    expect(view.getByText(`v${packageJson.version}`)).toBeTruthy();
    expect(view.getByRole("button", { name: /首页/ }).className).toContain("active");
    fireEvent.click(view.getByRole("button", { name: /下载管理/ }));
    expect(onNavigate).toHaveBeenCalledWith("queue");
    expect(view.getByRole("button", { name: /新剧监听 3/ })).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: /新剧监听/ }));
    expect(onNavigate).toHaveBeenCalledWith("monitor");
    const platform = view.getByRole("button", { name: "平台视频管理" });
    expect(platform.nextElementSibling?.textContent).toBe("设置");
    fireEvent.click(platform);
    expect(onNavigate).toHaveBeenCalledWith("platformVideos");
    fireEvent.click(view.getByRole("button", { name: "设置" }));
    expect(onNavigate).toHaveBeenCalledWith("settings");
  });
});
