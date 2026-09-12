import json
import unittest
from unittest.mock import AsyncMock, Mock, patch
from urllib.parse import parse_qs, urlparse

from core.device_pool import DeviceEntry
from core.http_client import PureSignedClient
from endpoints.duanju import _duanju_search_cache, duanju_search
from tests.test_duanju_extended import request_for


class SearchDeviceFailoverTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        _duanju_search_cache.clear()

    async def run_search(self, responses, offset=0, passback=""):
        title = "山河尽罢，共赴良辰"
        client = PureSignedClient()
        devices = [DeviceEntry(f"device-{n}", f"install-{n}", "test-key") for n in range(8)]
        client._get_device_with_auto_register = AsyncMock(side_effect=devices)
        client._register_fresh_device = AsyncMock(return_value=devices[-1])
        calls = []

        async def upstream(url, *args, **kwargs):
            query = parse_qs(urlparse(url).query, keep_blank_values=True)
            self.assertEqual(query["query"], [title])
            self.assertEqual(query["offset"], [str(offset)])
            self.assertEqual(query["passback"], [passback if offset else ""])
            calls.append(kwargs["device"].device_id)
            return {"ok": True, "upstream": responses[min(len(calls) - 1, len(responses) - 1)]}

        client._do_call = upstream
        pool = Mock()
        try:
            with patch("core.http_client.device_pool", pool), patch("core.http_client._trigger_ensure_pool"), patch("core.http_client.asyncio.sleep", new_callable=AsyncMock):
                response = await duanju_search(request_for(client), key=title, offset=offset,
                                               passback=passback, content_type="manju")
            return response, calls, pool
        finally:
            await client.close()

    async def test_missing_manju_tab_switches_device_instead_of_caching_empty_results(self):
        missing = {"code": 0, "search_tabs": [{"tab_type": 11, "data": []}]}
        valid = {"code": 0, "search_tabs": [{"tab_type": 19, "data": [{"video_data": [{
            "series_id": "found-series", "title": "山河尽罢，共赴良辰", "content_type": 1004,
        }]}]}]}
        response, calls, pool = await self.run_search([missing, valid], offset=10, passback="page+2/token")
        self.assertEqual([item["title"] for item in json.loads(response.body)["data"]["items"]], ["山河尽罢，共赴良辰"])
        self.assertEqual(calls, ["device-0", "device-1"])
        pool.report_failure.assert_called_once_with("device-0")
        pool.report_success.assert_called_once_with("device-1")

    async def test_valid_empty_tab_is_a_real_empty_search_without_device_churn(self):
        response, calls, pool = await self.run_search([{"code": 0, "search_tabs": [{"tab_type": 19, "data": []}]}])
        self.assertEqual(response.status_code, 200)
        self.assertEqual(json.loads(response.body)["data"]["items"], [])
        self.assertEqual(len(calls), 1)
        pool.report_failure.assert_not_called()

    async def test_persistent_missing_tab_is_an_error_and_is_not_cached(self):
        missing = {"code": 0, "search_tabs": [{"tab_type": 11, "data": []}]}
        response, calls, _ = await self.run_search([missing])
        self.assertEqual(response.status_code, 502)
        self.assertIn("漫剧搜索分类", json.loads(response.body)["msg"])
        self.assertEqual(len(calls), 8)
        _, next_calls, _ = await self.run_search([missing])
        self.assertEqual(len(next_calls), 8)
