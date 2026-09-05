import { describe, expect, it } from "vitest";
import { createGroupedPagingState, fillUniqueGroup } from "./groupedPaging";

type Item = { bookId: string; title: string };

function page(start: number, count: number, hasMore = true) {
  return {
    items: Array.from({ length: count }, (_, offset) => ({
      bookId: `book-${start + offset}`,
      title: `资讯 ${start + offset}`,
    })),
    nextCursor: start + count,
    hasMore,
  };
}

describe("fillUniqueGroup", () => {
  it("reveals exactly twenty unique items while retaining overfetch", async () => {
    const pages = [
      page(1, 6),
      page(7, 6),
      page(13, 6),
      page(19, 6),
      page(25, 6),
      page(31, 10, false),
    ];
    const fetchPage = async () => pages.shift()!;

    const first = await fillUniqueGroup(
      createGroupedPagingState<Item, number | null>(null),
      fetchPage,
    );

    expect(first.visible).toHaveLength(20);
    expect(first.buffer).toHaveLength(4);

    const second = await fillUniqueGroup(first.state, fetchPage);

    expect(second.visible).toHaveLength(40);
    expect(second.visible[39].bookId).toBe("book-40");
    expect(second.state.hasMore).toBe(false);
  });

  it("deduplicates and keeps fetching until twenty accepted exact matches", async () => {
    const pages = [
      {
        items: [
          ...page(1, 10).items,
          { bookId: "exact-1", title: "星河之恋" },
          { bookId: "exact-1", title: "星河之恋" },
        ],
        nextCursor: 1,
        hasMore: true,
      },
      {
        items: Array.from({ length: 19 }, (_, index) => ({
          bookId: `exact-${index + 2}`,
          title: "星河之恋",
        })),
        nextCursor: 2,
        hasMore: false,
      },
    ];

    const result = await fillUniqueGroup(
      createGroupedPagingState<Item, number>(0),
      async () => pages.shift()!,
      { include: (item) => item.title === "星河之恋" },
    );

    expect(result.visible).toHaveLength(20);
    expect(new Set(result.visible.map((item) => item.bookId)).size).toBe(20);
    expect(result.state.hasMore).toBe(false);
  });

  it("stops after the bounded upstream request count", async () => {
    let calls = 0;
    const result = await fillUniqueGroup(
      createGroupedPagingState<Item, number>(0),
      async () => {
        calls += 1;
        return { items: [], nextCursor: calls, hasMore: true };
      },
      { maxRequests: 3 },
    );

    expect(calls).toBe(3);
    expect(result.visible).toEqual([]);
    expect(result.state.hasMore).toBe(true);
  });

  it("caps a feed at one hundred items even when upstream keeps returning pages", async () => {
    let state = createGroupedPagingState<Item, number>(0);
    let calls = 0;

    for (let group = 0; group < 5; group += 1) {
      const result = await fillUniqueGroup(
        state,
        async (cursor) => {
          calls += 1;
          return page(cursor + 1, 20, true);
        },
        { maxItems: 100 },
      );
      state = result.state;
    }

    expect(state.allItems).toHaveLength(100);
    expect(state.visibleCount).toBe(100);
    expect(state.hasMore).toBe(false);
    expect(calls).toBe(5);

    const afterCap = await fillUniqueGroup(state, async () => {
      throw new Error("must not fetch past NO.100");
    }, { maxItems: 100 });
    expect(afterCap.visible).toHaveLength(100);
  });
});
