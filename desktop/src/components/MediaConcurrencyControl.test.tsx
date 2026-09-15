import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { MediaConcurrencyControl } from "./MediaConcurrencyControl";
afterEach(cleanup);

it("exposes automatic and one through ten slots, persists before allowing another update", async () => {
  let resolve!: () => void;
  const save = vi.fn(() => new Promise<void>(done => { resolve = done; }));
  const view = render(<MediaConcurrencyControl value={0} onChange={save} />);
  const select = view.getByRole("combobox", { name: "AI 同时处理" });
  expect(view.getAllByRole("option")).toHaveLength(11);
  expect(view.getByRole("option", { name: "自动（最多 5 个）" })).toHaveProperty("value", "0");
  fireEvent.change(select, { target: { value: "5" } });
  expect(save).toHaveBeenCalledWith(5);
  expect(select).toHaveProperty("disabled", true);
  await act(async () => resolve());
  view.rerender(<MediaConcurrencyControl value={5} onChange={save} />);
  expect(select).toHaveProperty("value", "5");
  expect(select).toHaveProperty("disabled", false);
});

it("keeps the saved selection and shows a failed update", async () => {
  const view = render(<MediaConcurrencyControl value={5} onChange={vi.fn().mockRejectedValue(new Error("disk full"))} />);
  await act(async () => fireEvent.change(view.getByRole("combobox"), { target: { value: "3" } }));
  expect(view.getByRole("alert").textContent).toContain("保存失败");
  expect(view.getByRole("combobox")).toHaveProperty("value", "5");
});

it("limits macOS choices to automatic or one through five", () => {
  const view = render(<MediaConcurrencyControl max={5} value={0} onChange={vi.fn()} />);
  expect(view.getAllByRole("option").map(o => (o as HTMLOptionElement).value)).toEqual(["0", "1", "2", "3", "4", "5"]);
});
