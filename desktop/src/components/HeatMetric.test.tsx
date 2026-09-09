import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { HeatMetric } from "./HeatMetric";

afterEach(cleanup);
describe("heat presentation", () => {
  it.each([[0, "0", "0"], [9999, "9999", "9,999"], [12500, "1.25万", "12,500"], [61300000, "6130万", "61,300,000"], [101220000, "1.01亿", "101,220,000"], [99999999, "1亿", "99,999,999"]])("keeps readable heat and the full count for %s", (value, compact, full) => {
    const view = render(<HeatMetric value={value as number} />);
    expect(view.container.textContent).toBe(`🔥 热度 ${compact}`);
    expect(view.container.firstElementChild?.getAttribute("title")).toContain(full);
  });
  it.each([undefined, NaN, Infinity, -1])("explicitly marks unavailable heat %s", value => {
    const view = render(<HeatMetric value={value} />);
    expect(view.container.textContent).toBe("热度 暂缺");
    expect(view.container.textContent).not.toContain("🔥");
  });
});
