import { fetchDiscovery, fetchDiscoveryMore, fetchWebCategory } from "../api";
import type { ContentType, DiscoveryPage } from "../types";

const liveApi = { fetchDiscovery, fetchDiscoveryMore, fetchWebCategory };
type HomePage = DiscoveryPage & { recommendationHasMore?: boolean; catalogNextPage?: number };

export async function fetchHomeDiscovery(type: ContentType, api = liveApi): Promise<HomePage> {
  const page = await api.fetchDiscovery(type);
  return { ...page, recommendationHasMore: page.hasMore, catalogNextPage: 1, hasMore: true };
}

export async function fetchHomeDiscoveryMore(type: ContentType, page: HomePage, api = liveApi): Promise<HomePage> {
  if (!page.catalogNextPage) return api.fetchDiscoveryMore(type, page);
  if (page.recommendationHasMore) {
    const next = await api.fetchDiscoveryMore(type, page);
    return { ...next, recommendationHasMore: next.hasMore, catalogNextPage: page.catalogNextPage, hasMore: true };
  }
  // The recommendation session can end after only a few items. Continue through
  // the matching website video library without repeating page 1 or mixing types.
  const next = await api.fetchWebCategory(type, {
    background: "", topic: "", setting: "", gender: "2", time: "0", sort_type: "1",
  }, page.catalogNextPage);
  if (next.hasMore && next.nextPage <= page.catalogNextPage) throw new Error("剧库分页未前进，请重试");
  return { ...page, items: next.items, recommendationHasMore: false,
    catalogNextPage: next.nextPage, hasMore: next.hasMore };
}
