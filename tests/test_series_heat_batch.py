import unittest
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

import httpx
from fastapi import FastAPI

from endpoints import duanju


class SeriesHeatBatchTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        duanju._duanju_series_metadata_cache.clear()
        app = FastAPI()
        app.state.client = SimpleNamespace()
        app.include_router(duanju.router, prefix='/api')
        self.client = httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url='http://test')

    async def asyncTearDown(self):
        await self.client.aclose()

    async def test_deduplicates_caches_by_id_and_preserves_zero_and_missing(self):
        with patch.object(duanju, '_fetch_series_metrics_batch', new=AsyncMock(return_value={
            '101': {'hot_count': 0, 'play_count': 9999}, '102': {'hot_count': 12000},
        })) as upstream:
            result = await self.client.get('/api/duanju/series-metrics-batch?series_ids=101,102,101,103')
            self.assertEqual(result.status_code, 200)
            items = result.json()['data']['items']
            self.assertEqual([item['series_id'] for item in items], ['101', '102', '103'])
            self.assertEqual(items[0]['hot_count'], 0)
            self.assertEqual(items[1]['hot_count'], 12000)
            self.assertNotIn('hot_count', items[2])
            self.assertEqual(upstream.call_args.args[1], ['101', '102', '103'])
            await self.client.get('/api/duanju/series-metrics-batch?series_ids=103,102')
            self.assertEqual(upstream.await_count, 1)
            await self.client.get('/api/duanju/series-metrics-batch?series_ids=101,104')
            self.assertEqual(upstream.call_args.args[1], ['104'])

    async def test_failure_remains_retryable(self):
        with patch.object(duanju, '_fetch_series_metrics_batch', new=AsyncMock(side_effect=[
            RuntimeError('private upstream context'), {'101': {'hot_count': 123}},
        ])):
            result = await self.client.get('/api/duanju/series-metrics-batch?series_ids=101')
            self.assertEqual(result.status_code, 502)
            self.assertNotIn('private', result.text)
            retry = await self.client.get('/api/duanju/series-metrics-batch?series_ids=101')
            self.assertEqual(retry.json()['data']['items'][0]['hot_count'], 123)

    async def test_rejects_invalid_or_unbounded_batches_before_querying_upstream(self):
        with patch.object(duanju, '_fetch_series_metrics_batch', new=AsyncMock()) as upstream:
            for ids in ['', '1,,2', 'a', '1,2/3', '1' * 25, ','.join(str(i) for i in range(41))]:
                result = await self.client.get('/api/duanju/series-metrics-batch', params={'series_ids': ids})
                self.assertIn(result.status_code, (400, 422))
            upstream.assert_not_awaited()
