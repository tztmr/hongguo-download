export type GroupedPage<T, Cursor> = {
  items: T[];
  nextCursor: Cursor;
  hasMore: boolean;
};

export type GroupedPagingState<T, Cursor> = {
  allItems: T[];
  visibleCount: number;
  cursor: Cursor;
  hasMore: boolean;
};

export type GroupedPagingOptions<T> = {
  groupSize?: number;
  maxItems?: number;
  maxRequests?: number;
  keyOf?: (item: T) => string;
  include?: (item: T) => boolean;
};

export function createGroupedPagingState<T, Cursor>(
  cursor: Cursor,
): GroupedPagingState<T, Cursor> {
  return { allItems: [], visibleCount: 0, cursor, hasMore: true };
}

export async function fillUniqueGroup<T, Cursor>(
  state: GroupedPagingState<T, Cursor>,
  fetchPage: (cursor: Cursor) => Promise<GroupedPage<T, Cursor>>,
  options: GroupedPagingOptions<T> = {},
) {
  const groupSize = options.groupSize ?? 20;
  const maxItems = options.maxItems ?? Number.POSITIVE_INFINITY;
  const maxRequests = options.maxRequests ?? 10;
  const keyOf = options.keyOf ?? ((item: T) => String((item as { bookId?: unknown }).bookId ?? ""));
  const include = options.include ?? (() => true);
  const target = Math.min(state.visibleCount + groupSize, maxItems);
  const allItems = state.allItems.slice(0, maxItems);
  const seen = new Set(allItems.map(keyOf));
  let cursor = state.cursor;
  let hasMore = state.hasMore;
  let requests = 0;

  while (allItems.length < target && hasMore && requests < maxRequests) {
    const page = await fetchPage(cursor);
    requests += 1;
    cursor = page.nextCursor;
    hasMore = page.hasMore;
    for (const item of page.items) {
      if (!include(item)) continue;
      const key = keyOf(item);
      if (!key || seen.has(key)) continue;
      seen.add(key);
      allItems.push(item);
      if (allItems.length >= maxItems) break;
    }
  }

  if (allItems.length >= maxItems) hasMore = false;

  const visibleCount = Math.min(target, allItems.length);
  const nextState: GroupedPagingState<T, Cursor> = {
    allItems,
    visibleCount,
    cursor,
    hasMore,
  };
  return {
    state: nextState,
    visible: allItems.slice(0, visibleCount),
    buffer: allItems.slice(visibleCount),
    requests,
  };
}
