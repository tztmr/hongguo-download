import asyncio
import unittest
from unittest.mock import patch
import httpx
from core.video_download import FIRST_BYTE_WAIT, download_video


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
            await request.extensions['trace']('http11.send_request_headers.started', {})
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
            await request.extensions['trace']('http11.send_request_headers.started', {})
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

    async def test_sustained_low_rate_does_not_download_duplicate_copy(self):
        requested = []
        class TrickleStream(httpx.AsyncByteStream):
            async def __aiter__(self):
                for _ in range(40):
                    yield b'v' * 128
                    await asyncio.sleep(.002)
        async def handle(request):
            requested.append(request.url.host)
            if request.url.host == 'a.test':
                return httpx.Response(200, stream=TrickleStream())
            return httpx.Response(200, content=b'backup' * 1024)
        # Speed alone cannot distinguish a poor CDN from shared network capacity.
        async with httpx.AsyncClient(transport=httpx.MockTransport(handle)) as client:
            result = await asyncio.wait_for(download_video(['https://a.test/v', 'https://b.test/v'], client), 1)
        self.assertEqual(result, b'v' * 5120)
        self.assertEqual(requested, ['a.test'])

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
        with patch('core.video_download.FIRST_BYTE_WAIT', .01):
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


class VideoDownloadPoolTests(unittest.IsolatedAsyncioTestCase):
    """Exercise connection admission/cleanup with real sockets, not mocked pools."""

    async def asyncSetUp(self):
        self.handlers = set()
        self.release = asyncio.Event()
        self.requested = []
        self.primary_delay = 0

        async def serve(reader, writer):
            task = asyncio.current_task()
            self.handlers.add(task)
            try:
                header = await reader.readuntil(b'\r\n\r\n')
                path = header.split(b' ')[1].decode()
                self.requested.append(path)
                if path == '/primary' and self.primary_delay:
                    await asyncio.sleep(self.primary_delay)
                writer.write(b'HTTP/1.1 200 OK\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n')
                await writer.drain()
                if path == '/hold':
                    await self.release.wait()
                writer.write(b'v' * 4096)
                await writer.drain()
            except (ConnectionError, asyncio.IncompleteReadError, asyncio.CancelledError):
                pass
            finally:
                writer.close()
                try:
                    await writer.wait_closed()
                except ConnectionError:
                    pass
                self.handlers.discard(task)

        self.server = await asyncio.start_server(serve, '127.0.0.1', 0)
        self.base = f'http://127.0.0.1:{self.server.sockets[0].getsockname()[1]}'

    async def asyncTearDown(self):
        self.release.set()
        self.server.close()
        await self.server.wait_closed()
        pending = list(self.handlers)
        for task in pending:
            task.cancel()
        await asyncio.gather(*pending, return_exceptions=True)

    def client(self, **kwargs):
        return httpx.AsyncClient(
            trust_env=False, limits=httpx.Limits(max_connections=1),
            timeout=httpx.Timeout(1, pool=.06), **kwargs,
        )

    async def test_pool_timeout_retries_same_source_after_connection_releases(self):
        attempts = []
        retried = asyncio.Event()
        self.primary_delay = .05

        async def request_hook(request):
            if request.url.path != '/hold':
                attempts.append(request.url.path)
                if len(attempts) >= 2:
                    retried.set()

        with patch('core.video_download.FIRST_BYTE_WAIT', .01):
            async with self.client(event_hooks={'request': [request_hook]}) as client:
                async with client.stream('GET', self.base + '/hold') as occupied:
                    task = asyncio.create_task(download_video(
                        [self.base + '/primary', self.base + '/backup'], client,
                    ))
                    try:
                        await asyncio.wait_for(retried.wait(), 2)
                        self.assertEqual(attempts, ['/primary', '/primary'])
                        self.assertEqual(self.requested, ['/hold'])
                        # The 10 ms deadline only stresses connection admission.
                        # Once admitted, a healthy real socket (especially on
                        # Windows) may need longer; use the production allowance.
                        with patch('core.video_download.FIRST_BYTE_WAIT', FIRST_BYTE_WAIT):
                            await occupied.aclose()
                            self.assertEqual(await asyncio.wait_for(task, 2), b'v' * 4096)
                    finally:
                        task.cancel()
                        await asyncio.gather(task, return_exceptions=True)
        self.assertEqual(attempts, ['/primary', '/primary'])
        self.assertEqual(self.requested, ['/hold', '/primary'])

    async def test_persistent_pool_pressure_is_bounded_and_not_a_cdn_failure(self):
        async with self.client() as client:
            async with client.stream('GET', self.base + '/hold'):
                with self.assertRaisesRegex(RuntimeError, '下载连接繁忙') as raised:
                    await asyncio.wait_for(download_video(
                        [self.base + '/primary?token=secret', self.base + '/backup'], client,
                    ), 3)
                self.assertNotIn('secret', str(raised.exception))
            # Failed queued attempts must not leave a connection/queue entry behind.
            self.assertEqual(await download_video([self.base + '/next'], client), b'v' * 4096)

    async def test_cancel_waiting_download_never_starts_a_backup_or_holds_a_connection(self):
        attempted = asyncio.Event()
        attempts = []

        async def request_hook(request):
            if request.url.path != '/hold':
                attempts.append(request.url.path)
                attempted.set()

        async with self.client(event_hooks={'request': [request_hook]}) as client:
            async with client.stream('GET', self.base + '/hold'):
                task = asyncio.create_task(download_video(
                    [self.base + '/primary', self.base + '/backup'], client,
                ))
                await asyncio.wait_for(attempted.wait(), 1)
                task.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await task
            self.assertEqual(await download_video([self.base + '/next'], client), b'v' * 4096)
        self.assertEqual(attempts, ['/primary', '/next'])

    async def test_cancel_active_stream_releases_its_connection_for_next_download(self):
        async with self.client() as client:
            task = asyncio.create_task(download_video([self.base + '/hold'], client))
            try:
                async with asyncio.timeout(1):
                    while '/hold' not in self.requested:
                        await asyncio.sleep(.005)
                task.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await task
                self.assertEqual(await download_video([self.base + '/next'], client), b'v' * 4096)
            finally:
                task.cancel()
                await asyncio.gather(task, return_exceptions=True)
