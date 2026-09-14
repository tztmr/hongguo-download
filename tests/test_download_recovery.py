import asyncio
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import AsyncMock, patch

import httpx
from fastapi import FastAPI
from endpoints.duanju import router
from core.response import error


class DownloadRecoveryTest(unittest.IsolatedAsyncioTestCase):
    async def request(self, app):
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url='http://test') as c:
            return await c.get('/api/duanju/download?item_id=7681905302022655038&definition=auto')

    async def test_transient_cdn_failure_refreshes_only_this_episodes_model_and_retries(self):
        from fastapi.responses import Response
        app = FastAPI(); app.include_router(router, prefix='/api')
        run = AsyncMock(side_effect=[error('CDN 失败', code=-8, status_code=502), Response(b'mp4', media_type='video/mp4')])
        with patch('endpoints.duanju._duanju_download_once', run), patch('endpoints.duanju._duanju_video_cache.delete') as delete:
            response = await self.request(app)
        self.assertEqual(response.status_code, 200)
        self.assertEqual(run.await_count, 2)
        delete.assert_called_once()
        self.assertEqual(delete.call_args.args[0], 'duanju_content|item_id=7681905302022655038')

    async def test_uncaught_error_becomes_structured_stage_error_with_safe_diagnostic(self):
        app = FastAPI(); app.include_router(router, prefix='/api')
        async def fail(request, *args):
            request.state.download_stage = '视频解密'
            raise OSError('secret=https://cdn.invalid?token=private')
        with tempfile.TemporaryDirectory() as directory, \
                patch.dict('os.environ', {'HONGGUO_DATA_DIR': directory}), \
                patch('endpoints.duanju._duanju_download_once', fail):
            response = await self.request(app)
            self.assertEqual(response.status_code, 502)
            self.assertIn('视频解密', response.json()['msg'])
            self.assertIn('OSError', response.json()['msg'])
            logs = list((Path(directory) / 'logs').glob('download-errors*'))
            self.assertTrue(logs)
            self.assertNotIn('private', logs[0].read_text(encoding='utf-8'))
            self.assertNotIn('private', response.text)

    async def test_ten_requests_bound_in_memory_downloads_and_all_finish(self):
        from fastapi.responses import Response
        app = FastAPI(); app.include_router(router, prefix='/api')
        active = peak = 0
        async def run(*args):
            nonlocal active, peak
            active += 1; peak = max(peak, active)
            await asyncio.sleep(.01)
            active -= 1
            return Response(b'mp4', media_type='video/mp4')
        with patch('endpoints.duanju._duanju_download_once', run):
            responses = await asyncio.gather(*(self.request(app) for _ in range(10)))
        self.assertLessEqual(peak, 3)
        self.assertGreater(peak, 1)
        self.assertTrue(all(r.status_code == 200 for r in responses))
