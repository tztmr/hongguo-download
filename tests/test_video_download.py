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

    async def test_primary_stalling_after_first_chunk_uses_backup(self):
        closed = asyncio.Event()
        class StallingStream(httpx.AsyncByteStream):
            async def __aiter__(self):
                yield b'v' * 1024
                await asyncio.Event().wait()
            async def aclose(self):
                closed.set()
        async def handle(request):
            if request.url.host == 'a.test':
                return httpx.Response(200, stream=StallingStream())
            return httpx.Response(200, content=b'backup' * 1024)
        with patch('core.video_download.STALL_WAIT', .02):
            async with httpx.AsyncClient(transport=httpx.MockTransport(handle)) as client:
                result = await asyncio.wait_for(download_video(['https://a.test/v', 'https://b.test/v'], client), 1)
        self.assertEqual(result, b'backup' * 1024)
        self.assertTrue(closed.is_set())

    async def test_continuously_trickling_primary_uses_faster_backup(self):
        class TrickleStream(httpx.AsyncByteStream):
            async def __aiter__(self):
                while True:
                    yield b'v'
                    await asyncio.sleep(.002)
        async def handle(request):
            if request.url.host == 'a.test':
                return httpx.Response(200, stream=TrickleStream())
            return httpx.Response(200, content=b'backup' * 1024)
        with patch('core.video_download.SLOW_WINDOW', .03):
            async with httpx.AsyncClient(transport=httpx.MockTransport(handle)) as client:
                result = await asyncio.wait_for(download_video(['https://a.test/v', 'https://b.test/v'], client), 1)
        self.assertEqual(result, b'backup' * 1024)

    async def test_healthy_stream_beyond_first_byte_deadline_never_hedges(self):
        requested = []
        class HealthyStream(httpx.AsyncByteStream):
            async def __aiter__(self):
                for _ in range(12):
                    yield b'v' * 65536
                    await asyncio.sleep(.005)
        async def handle(request):
            requested.append(request.url.host)
            return httpx.Response(200, stream=HealthyStream())
        with patch('core.video_download.FIRST_BYTE_WAIT', .01), patch('core.video_download.SLOW_WINDOW', .03):
            async with httpx.AsyncClient(transport=httpx.MockTransport(handle)) as client:
                result = await download_video(['https://a.test/v', 'https://b.test/v'], client)
        self.assertEqual(len(result), 12 * 65536)
        self.assertEqual(requested, ['a.test'])

    async def test_hedging_is_bounded_and_invalid_backup_keeps_primary(self):
        active = 0
        peak = 0
        requested = []
        primary_done = asyncio.Event()
        class PrimaryStream(httpx.AsyncByteStream):
            async def __aiter__(self):
                yield b'v' * 1024
                await primary_done.wait()
                yield b'v' * 1024
            async def aclose(self):
                nonlocal active
                active -= 1
        async def handle(request):
            nonlocal active, peak
            requested.append(request.url.host)
            active += 1
            peak = max(peak, active)
            if request.url.host == 'a.test':
                return httpx.Response(200, stream=PrimaryStream())
            await asyncio.sleep(.03)
            active -= 1
            if request.url.host == 'c.test':
                primary_done.set()
            return httpx.Response(200, content=b'short')
        with patch('core.video_download.STALL_WAIT', .01):
            async with httpx.AsyncClient(transport=httpx.MockTransport(handle)) as client:
                result = await asyncio.wait_for(download_video(['https://a.test/v', 'https://b.test/v', 'https://c.test/v'], client), 1)
        self.assertEqual(result, b'v' * 2048)
        self.assertEqual(peak, 2)
        self.assertEqual(active, 0)
        self.assertEqual(requested, ['a.test', 'b.test', 'c.test'])
