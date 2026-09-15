import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { fetchDiscovery, fetchDiscoveryMore } from "./api";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const data = { items: [], cell_id: "cell", next_offset: 6, has_more: true, session_id: "session", plan_id: "plan", filter_ids: "series-1", selected_items: "cate_20" };
beforeEach(() => vi.mocked(invoke).mockReset());
it("only offers pagination when the first response has the required upstream cursor", async () => {
  vi.mocked(invoke).mockResolvedValueOnce({ ...data, session_id: "" }).mockResolvedValueOnce(data);
  expect((await fetchDiscovery("manju")).hasMore).toBe(false);
  expect((await fetchDiscovery("manju")).hasMore).toBe(true);
});
it("carries category and all upstream paging state, then stops on a repeated offset", async () => {
  vi.mocked(invoke).mockResolvedValueOnce(data).mockResolvedValueOnce({ ...data, session_id: "new-session" });
  const first = await fetchDiscovery("manju");
  const next = await fetchDiscoveryMore("manju", first);
  const query = new URLSearchParams((vi.mocked(invoke).mock.lastCall![1] as { path: string }).path.split("?")[1]);
  expect(Object.fromEntries(query)).toEqual({ content_type: "manju", cell_id: "cell", offset: "6", session_id: "session", plan_id: "plan", filter_ids: "series-1", selected_items: "cate_20" });
  expect(next.hasMore).toBe(false);
  expect(next.sessionId).toBe("new-session");
});
