import asyncio
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

import httpx
from fastapi import FastAPI
from endpoints.duanju import duanju_download, router
from core.response import error
from core.video_download import VideoPoolBusy


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

    async def test_pool_busy_returns_retryable_503_without_refreshing_playback_urls(self):
        app = FastAPI(); app.include_router(router, prefix='/api')
        app.state.download_slots = asyncio.Semaphore(1)
        source = {'urls': ['https://invalid.test/video?token=secret'], 'spade_a': 'key', 'definition': '720p'}
        model = AsyncMock(return_value={'sources': [source]})
        download = AsyncMock(side_effect=VideoPoolBusy())
        with patch('endpoints.duanju._fetch_video_model', model), \
                patch('endpoints.duanju.derive_key_from_spade_a', return_value='key'), \
                patch('endpoints.duanju._download_encrypted', download), \
                patch('endpoints.duanju._duanju_video_cache.delete') as delete:
            response = await self.request(app)
        self.assertEqual(response.status_code, 503)
        self.assertEqual(response.json()['code'], -11)
        self.assertEqual(response.headers['retry-after'], '5')
        self.assertIn('下载连接繁忙', response.json()['msg'])
        self.assertNotIn('secret', response.text)
        model.assert_awaited_once()
        download.assert_awaited_once()
        delete.assert_not_called()
        await asyncio.wait_for(app.state.download_slots.acquire(), .5)
        app.state.download_slots.release()

    async def test_disconnected_waiter_does_not_start_downloading_or_consume_slot(self):
        disconnected = asyncio.Event()
        app = FastAPI()
        app.state.download_slots = asyncio.Semaphore(0)

        async def is_disconnected():
            return disconnected.is_set()

        request = SimpleNamespace(app=app, state=SimpleNamespace(), is_disconnected=is_disconnected)
        run = AsyncMock()
        with patch('endpoints.duanju._duanju_download_once', run):
            task = asyncio.create_task(duanju_download(request, 'episode', 'auto', False, False))
            try:
                await asyncio.sleep(.01)
                disconnected.set()
                with self.assertRaises(asyncio.CancelledError):
                    await asyncio.wait_for(task, 1)
                run.assert_not_awaited()
            finally:
                task.cancel()
                await asyncio.gather(task, return_exceptions=True)
        app.state.download_slots.release()
        await asyncio.wait_for(app.state.download_slots.acquire(), .5)
        app.state.download_slots.release()

    async def test_regular_download_disconnect_cancels_cdn_and_next_episode_can_run(self):
        from fastapi.responses import Response
        disconnected = asyncio.Event()
        started = asyncio.Event()
        closed = asyncio.Event()
        app = FastAPI()
        app.state.download_slots = asyncio.Semaphore(1)

        async def is_disconnected():
            return disconnected.is_set()

        async def download(*args):
            started.set()
            try:
                await asyncio.Event().wait()
            finally:
                closed.set()

        request = SimpleNamespace(app=app, state=SimpleNamespace(), is_disconnected=is_disconnected)
        source = {'urls': ['https://invalid.test/video'], 'spade_a': 'key', 'definition': '720p'}
        with patch('endpoints.duanju._fetch_video_model', AsyncMock(return_value={'sources': [source]})), \
                patch('endpoints.duanju.derive_key_from_spade_a', return_value='key'), \
                patch('endpoints.duanju._download_encrypted', download), \
                patch('endpoints.duanju.decrypt_mp4') as decrypt:
            task = asyncio.create_task(duanju_download(request, 'episode', 'auto', False, False))
            try:
                await asyncio.wait_for(started.wait(), 1)
                disconnected.set()
                with self.assertRaises(asyncio.CancelledError):
                    await asyncio.wait_for(task, 1)
                self.assertTrue(closed.is_set())
                decrypt.assert_not_called()
            finally:
                task.cancel()
                await asyncio.gather(task, return_exceptions=True)
        disconnected.clear()
        with patch('endpoints.duanju._duanju_download_once', AsyncMock(return_value=Response(b'next'))):
            response = await asyncio.wait_for(duanju_download(request, 'next', 'auto', False, False), 1)
            self.assertEqual(response.body, b'next')
