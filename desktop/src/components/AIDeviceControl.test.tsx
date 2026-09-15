import { act, fireEvent, render } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { AIDeviceControl } from "./AIDeviceControl";

it("blocks duplicate saves and leaves a failed CPU selection retryable", async () => {
  let reject!: (error: Error) => void;
  const onChange = vi.fn().mockImplementationOnce(() => new Promise((_resolve, fail) => { reject = fail; })).mockResolvedValue(undefined);
  const view = render(<AIDeviceControl value="auto" onChange={onChange} />);
  const select = view.getByRole("combobox", { name: "AI 计算设备" });
  fireEvent.change(select, { target: { value: "cpu" } });
  fireEvent.change(select, { target: { value: "cpu" } });
  expect(onChange).toHaveBeenCalledExactlyOnceWith("cpu");
  expect(select).toHaveProperty("disabled", true);
  await act(async () => reject(new Error("磁盘不可写")));
  expect(view.getByRole("alert").textContent).toContain("磁盘不可写");
  expect(select).toHaveProperty("value", "auto");
  fireEvent.change(select, { target: { value: "cpu" } });
  await act(async () => {});
  expect(onChange).toHaveBeenCalledTimes(2);
  expect(view.queryByRole("alert")).toBeNull();
});
