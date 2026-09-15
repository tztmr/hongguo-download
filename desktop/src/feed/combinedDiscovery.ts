import { errorMessage } from "../errors";
import type { ContentType, DiscoveryPage, RankPage, SeriesItem } from "../types";

export type HomeSource = ContentType | "ai";
export const homeSourceNames = { drama: "真人剧", manju: "漫剧", ai: "AI剧" };
export type CombinedDiscoveryPage = DiscoveryPage & {
  sources?: Partial<Record<ContentType, DiscoveryPage>>;
  aiPage?: RankPage;
  aiSeenCursors?: string[];
  sourceErrors?: Partial<Record<HomeSource, string>>;
};
const empty: DiscoveryPage = { items: [], categories: [], rankBoards: [], cellId: "", nextOffset: 0,
  sessionId: "", planId: "", filterIds: "", selectedItems: "", hasMore: false };

export async function combinedDiscovery(
  previous: CombinedDiscoveryPage | null,
  first: (type: ContentType) => Promise<DiscoveryPage>,
  more: (type: ContentType, page: DiscoveryPage) => Promise<DiscoveryPage>,
  ai?: (cursor?: string) => Promise<RankPage>,
  retryFailedOnly = false,
): Promise<CombinedDiscoveryPage> {
  const sources = { ...previous?.sources };
  const sourceErrors = { ...previous?.sourceErrors };
  let aiPage = previous?.aiPage;
  const aiSeenCursors = new Set(previous?.aiSeenCursors);
  const keys: HomeSource[] = ai ? ["drama", "manju", "ai"] : ["drama", "manju"];
  const pages = await Promise.all(keys.map(async type => {
    const failed = !!sourceErrors[type];
    // Failed sources wait for explicit retry while healthy sources keep paging.
    if (retryFailedOnly ? !failed : failed) return [];
    try {
      if (type === "ai") {
        if (aiPage && !aiPage.hasMore) return [];
        const result = await ai!(aiPage?.nextCursor);
        if (aiPage?.nextCursor) aiSeenCursors.add(aiPage.nextCursor);
        aiPage = { ...result, hasMore: result.hasMore && !!result.nextCursor && !aiSeenCursors.has(result.nextCursor) };
        delete sourceErrors[type];
        return result.items;
      }
      const cursor = sources[type];
      if (cursor && !cursor.hasMore) return [];
      const result = cursor ? await more(type, cursor) : await first(type);
      sources[type] = result;
      delete sourceErrors[type];
      return result.items;
    } catch (reason) {
      sourceErrors[type] = errorMessage(reason);
      return [];
    }
  }));
  const items: SeriesItem[] = [];
  const seen = new Set<string>();
  for (let index = 0; index < Math.max(0, ...pages.map(page => page.length)); index++) {
    for (const page of pages) {
      const item = page[index];
      if (item?.bookId && !seen.has(item.bookId)) { seen.add(item.bookId); items.push(item); }
    }
  }
  return { ...empty, items, sources, aiPage, aiSeenCursors: [...aiSeenCursors], sourceErrors,
    hasMore: keys.some(type => !sourceErrors[type] && (type === "ai" ? aiPage?.hasMore : sources[type]?.hasMore)) };
}

export function discoveryCategoryGroups(categories: DiscoveryPage["categories"]) {
  const groups = new Map<string, { id: string; name: string; items: Array<{ id: string; name: string }> }>();
  for (const item of categories) {
    if (!item.id || !item.name) continue;
    const name = item.group || "题材";
    const group = groups.get(name) ?? { id: name, name, items: [{ id: "all", name: "全部" }] };
    if (!group.items.some(option => option.id === item.id)) group.items.push({ id: item.id, name: item.name });
    groups.set(name, group);
  }
  return [...groups.values()];
}
