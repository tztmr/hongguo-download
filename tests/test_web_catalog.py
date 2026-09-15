import json
import unittest
from unittest.mock import AsyncMock, patch
from urllib.parse import parse_qs, urlparse

import httpx
from fastapi import FastAPI

from core.web_catalog import (
    CatalogFormatError,
    build_category_url,
    parse_category_page,
    parse_selector_html,
)
from endpoints import web_catalog


# Field names and enum values captured from the public website on 2026-09-09.
# The fixture retains no request identifiers, portraits, or story descriptions.
SELECTORS = [
    {"row_id": 1, "row_name": "全部背景", "items": [{"selector_item_id": "cate_757", "show_name": "现代"}]},
    {"row_id": 2, "row_name": "全部主题", "items": [{"selector_item_id": "cate_1021", "show_name": "现言"}]},
    {"row_id": 3, "row_name": "全部设定", "items": [{"selector_item_id": "cate_36", "show_name": "重生"}]},
    {"row_id": 4, "row_name": "全部受众", "items": [{"selector_item_id": "1", "show_name": "男频"}, {"selector_item_id": "0", "show_name": "女频"}]},
    {"row_id": 5, "row_name": "全部时间", "items": [{"selector_item_id": "1", "show_name": "7天内上新"}]},
    {"row_id": 6, "row_name": "全部推荐", "items": [{"selector_item_id": "2", "show_name": "最新"}, {"selector_item_id": "1", "show_name": "最热"}]},
]


def selector_html(rows=None):
    data = {"loaderData": {"category_layout": None, "category_page": {
        "req": {"webID": "must-not-be-returned"}, "isSuccess": True,
        "selectorList": SELECTORS if rows is None else rows,
    }}}
    return '<html><script>_ROUTER_DATA = ' + json.dumps(data) + ';</script></html>'


def modern_selector_html():
    return '<script>window._ROUTER_DATA = ' + json.dumps({"loaderData": {"category_$": {
        "isSuccess": True, "categoryRoute": {"contentType": "real-drama"},
        "selectorList": [{"row_id": 1, "items": [
            {"selector_item_id": "romance", "show_name": "爱情", "category_json_ids": ["5000"]},
            {"selector_item_id": "urban", "show_name": "都市", "category_json_ids": ["5022"]},
        ]}],
    }}}) + ';</script>'


def category_payload(page=1, total=25):
    return {
        "isSuccess": True, "pageNum": page, "pageSize": 24, "total": total,
        "recommendList": [{
            "series_id": "1234567890123456789", "series_name": "测试短剧",
            "series_cover": "https://example.invalid/cover.jpg", "series_intro": "简介",
            "episode_cnt": 2, "vid_list": ["2234567890123456789", "3234567890123456789"],
            "tags": ["现代", "重生", "现代", "\u0000甜宠"],
            "series_episode_info": {"episode_cnt": 2, "episode_total_cnt": 2, "series_status": 1},
        }],
    }


class WebCatalogParserTests(unittest.TestCase):
    def test_selector_rows_keep_dimensions_and_distinguish_zero_from_default(self):
        groups = parse_selector_html(selector_html())
        self.assertEqual([row["id"] for row in groups], ["background", "topic", "setting", "gender", "time", "sort_type"])
        self.assertEqual(groups[3]["items"], [
            {"id": "2", "name": "全部"}, {"id": "1", "name": "男频"}, {"id": "0", "name": "女频"},
        ])
        self.assertNotIn("webID", json.dumps(groups))

    def test_current_route_dictionary_preserves_slugs_and_rpc_category_ids(self):
        groups = parse_selector_html(modern_selector_html())
        self.assertEqual([group["id"] for group in groups], ["topic"])
        self.assertEqual(groups[0]["items"][1], {"id": "romance", "name": "爱情", "category_json_ids": ["5000"]})
        url = build_category_url(topic="romance", category_json_ids=["5000"], page=6)
        params = parse_qs(urlparse(url).query)
        self.assertEqual(params["content_type"], ["1"])
        self.assertEqual(params["category_json_ids"], ["5000"])
        self.assertNotIn("categories_v2", params)
        self.assertEqual(params["page_num"], ["6"])

    def test_category_url_keeps_combined_facets_and_time_enum(self):
        url = build_category_url(background="cate_757", topic="cate_1021", setting="cate_36", gender="0", time="1", sort_type="2", page=2)
        params = parse_qs(urlparse(url).query)
        self.assertEqual(params["categories_v2"], ["cate_1021,cate_36,cate_757"])
        self.assertEqual(params["min_first_visible_time"], ["1"])
        self.assertEqual(params["gender"], ["0"])
        self.assertEqual(params["page_num"], ["2"])

    def test_list_normalizes_download_ids_preserves_missing_metrics_and_paginates(self):
        data = parse_category_page(category_payload(), page=1)
        item = data["items"][0]
        self.assertEqual(item["series_id"], "1234567890123456789")
        self.assertEqual(item["first_vid"], "2234567890123456789")
        self.assertEqual(item["content_type"], 1)
        self.assertEqual(item["category_tags"], ["现代", "重生", "甜宠"])
        self.assertIsNone(item["online_time"])
        self.assertIsNone(item["hot_count"])
        self.assertEqual(data["next_page"], 2)
        self.assertTrue(data["has_more"])
        final = parse_category_page(category_payload(page=2), page=2)
        self.assertFalse(final["has_more"])
        self.assertIsNone(final["next_page"])

    def test_real_empty_result_is_valid_but_invalid_or_wrong_page_is_not(self):
        empty = category_payload(total=0)
        empty["recommendList"] = []
        self.assertEqual(parse_category_page(empty, page=1)["items"], [])
        for invalid in [{}, {**empty, "isSuccess": False}, {**empty, "pageNum": 2}, {**empty, "recommendList": None}]:
            with self.assertRaises(CatalogFormatError):
                parse_category_page(invalid, page=1)
        with self.assertRaises(CatalogFormatError):
            parse_selector_html('<script>doSomething()</script>')
        with self.assertRaises(CatalogFormatError):
            parse_selector_html(selector_html([]))


class WebCatalogEndpointTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        web_catalog._catalog_cache.clear()
        app = FastAPI()
        app.include_router(web_catalog.router, prefix="/api")
        self.client = httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url="http://test")

    async def asyncTearDown(self):
        await self.client.aclose()

    async def test_combined_filters_page_and_default_have_separate_cache_keys(self):
        async def fetch(url):
            if '/category' in url and '/api/' not in url:
                return selector_html()
            params = parse_qs(urlparse(url).query)
            return category_payload(page=int(params["page_num"][0]))

        with patch.object(web_catalog, "_fetch_public", new=AsyncMock(side_effect=fetch)) as upstream:
            path = '/api/duanju/web-category?background=cate_757&topic=cate_1021&setting=cate_36&gender=0&time=1&sort_type=2'
            response = await self.client.get(path)
            self.assertEqual(response.status_code, 200)
            self.assertEqual(response.json()["data"]["next_page"], 2)
            await self.client.get(path)
            await self.client.get(path + '&page=2')
            await self.client.get('/api/duanju/web-category')
            self.assertEqual(upstream.await_count, 4)  # One dictionary and three distinct result pages.

    async def test_mismatched_facet_and_unsupported_comics_are_rejected(self):
        with patch.object(web_catalog, "_fetch_public", new=AsyncMock(return_value=selector_html())) as upstream:
            response = await self.client.get('/api/duanju/web-category?topic=cate_757')
            self.assertEqual(response.status_code, 400)
            self.assertIn("主题", response.json()["msg"])
            unsupported = await self.client.get('/api/duanju/web-category?content_type=manju')
            self.assertEqual(unsupported.status_code, 400)
            self.assertIn("漫画", unsupported.json()["msg"])
            self.assertEqual(upstream.await_count, 1)

    async def test_upstream_failure_is_visible_and_not_cached_as_empty(self):
        with patch.object(web_catalog, "_fetch_public", new=AsyncMock(side_effect=[CatalogFormatError('官网返回格式已变化'), category_payload()])) as upstream:
            first = await self.client.get('/api/duanju/web-category')
            self.assertEqual(first.status_code, 502)
            self.assertNotEqual(first.json()["code"], 0)
            second = await self.client.get('/api/duanju/web-category')
            self.assertEqual(second.status_code, 200)
            self.assertEqual(upstream.await_count, 2)

    async def test_current_classification_validates_slug_and_forwards_rpc_ids_on_every_page(self):
        async def fetch(url):
            if '/api/' not in url:
                return modern_selector_html()
            return category_payload(page=int(parse_qs(urlparse(url).query)["page_num"][0]), total=800)
        with patch.object(web_catalog, "_fetch_public", new=AsyncMock(side_effect=fetch)) as upstream:
            groups = await self.client.get('/api/duanju/web-categories')
            self.assertEqual(groups.json()["data"]["groups"][0]["items"][1]["name"], "爱情")
            for number in (1, 6):
                response = await self.client.get(f'/api/duanju/web-category?topic=romance&page={number}')
                self.assertEqual(response.status_code, 200)
                params = parse_qs(urlparse(upstream.call_args.args[0]).query)
                self.assertEqual(params['category_json_ids'], ['5000'])
                self.assertEqual(params['page_num'], [str(number)])
                self.assertTrue(response.json()['data']['has_more'])
            for query in ('topic=unknown', 'background=cate_757'):
                response = await self.client.get('/api/duanju/web-category?' + query)
                self.assertEqual(response.status_code, 400)

    async def test_public_category_fetch_follows_the_website_redirect(self):
        def handler(request):
            if request.url.path == '/category':
                return httpx.Response(301, headers={'location': '/category/real-drama'})
            return httpx.Response(200, text=modern_selector_html())
        client_class = httpx.AsyncClient
        with patch.object(httpx, 'AsyncClient', side_effect=lambda **kwargs: client_class(transport=httpx.MockTransport(handler), **kwargs)):
            html = await web_catalog._fetch_public('https://hongguoduanju.com/category')
        self.assertEqual(parse_selector_html(html)[0]['items'][1]['id'], 'romance')

    async def test_api_validates_enum_and_page_before_upstream_request(self):
        with patch.object(web_catalog, "_fetch_public", new=AsyncMock()) as upstream:
            for query in ['page=0', 'gender=3', 'time=5', 'sort_type=9', 'background=https://invalid.test/']:
                response = await self.client.get('/api/duanju/web-category?' + query)
                self.assertEqual(response.status_code, 422)
            upstream.assert_not_awaited()


if __name__ == '__main__':
    unittest.main()
