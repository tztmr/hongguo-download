import asyncio
import unittest
from unittest.mock import patch
import httpx
from core.video_download import download_video


class VideoDownloadTests(unittest.IsolatedAsyncioTestCase):
    async def test_fast_primary_never_downloads_backup(self):
        urls = []
        async def handle(request):
            urls.append(str(request.url))
            return httpx.Response(200, content=b'v' * 4096)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handle)) as client:
            self.assertEqual(await download_video(['https://a.test/v', 'https://b.test/v'], client), b'v' * 4096)
        self.assertEqual(urls, ['https://a.test/v'])

    async def test_stalled_primary_hedges_and_is_cancelled_when_backup_finishes(self):
        cancelled = asyncio.Event()
        async def handle(request):
            if request.url.host == 'a.test':
                try:
                    await asyncio.Event().wait()
                finally:
                    cancelled.set()
            return httpx.Response(200, content=b'backup' * 1024)
        with patch('core.video_download.FIRST_BYTE_WAIT', .01):
            async with httpx.AsyncClient(transport=httpx.MockTransport(handle)) as client:
                result = await asyncio.wait_for(download_video(['https://a.test/v', 'https://b.test/v'], client), 1)
        self.assertEqual(result, b'backup' * 1024)
        self.assertTrue(cancelled.is_set())

    async def test_invalid_response_fails_over_and_all_failed_is_actionable(self):
        async def handle(request):
            return httpx.Response(200 if request.url.host == 'a.test' else 403, content=b'short')
        async with httpx.AsyncClient(transport=httpx.MockTransport(handle)) as client:
            with self.assertRaisesRegex(RuntimeError, '所有 CDN 下载失败'):
                await download_video(['https://a.test/v?secret=123', 'https://b.test/v'], client)

    async def test_cancelling_download_closes_active_primary_and_backup(self):
        started = 0
        closed = 0
        ready = asyncio.Event()
        async def handle(request):
            nonlocal started, closed
            started += 1
            if started == 2: ready.set()
            try:
                await asyncio.Event().wait()
            finally:
                closed += 1
        with patch('core.video_download.FIRST_BYTE_WAIT', .01):
            async with httpx.AsyncClient(transport=httpx.MockTransport(handle)) as client:
                job = asyncio.create_task(download_video(['https://a.test/v', 'https://b.test/v'], client))
                await asyncio.wait_for(ready.wait(), 1)
                job.cancel()
                with self.assertRaises(asyncio.CancelledError): await job
        self.assertEqual(closed, 2)
