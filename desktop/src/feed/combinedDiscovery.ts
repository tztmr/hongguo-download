import type { ContentType, DiscoveryPage } from "../types";
export type CombinedDiscoveryPage = DiscoveryPage & { sources?: Record<ContentType, DiscoveryPage> };
export async function combinedDiscovery(
  previous: CombinedDiscoveryPage | null,
  first: (type: ContentType) => Promise<DiscoveryPage>,
  more: (type: ContentType, page: DiscoveryPage) => Promise<DiscoveryPage>,
): Promise<CombinedDiscoveryPage> {
  const pages = await Promise.all((["drama", "manju"] as const).map(async (type) => {
    const cursor = previous?.sources?.[type];
    return cursor ? cursor.hasMore ? more(type, cursor) : { ...cursor, items: [] } : first(type);
  }));
  const items = [];
  for (let index = 0; index < Math.max(...pages.map((page) => page.items.length)); index++) {
    for (const page of pages) if (page.items[index]) items.push(page.items[index]);
  }
  return { ...pages[0], items, categories: [], hasMore: pages.some((page) => page.hasMore),
    sources: { drama: pages[0], manju: pages[1] } };
}
