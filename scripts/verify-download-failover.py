"""Exercise real HTTP streaming against a local stalled CDN (no user media)."""
import argparse
import asyncio
import importlib.util
from pathlib import Path
import sys
import time

import httpx

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from core.video_download import download_video

PAYLOAD = b'video-download-test' * 65536


async def measure(download):
    requests = []
    handlers = set()

    async def serve(reader, writer):
        task = asyncio.current_task()
        handlers.add(task)
        try:
            header = await reader.readuntil(b'\r\n\r\n')
            path = header.split(b' ')[1]
            requests.append(path.decode())
            writer.write(f'HTTP/1.1 200 OK\r\nContent-Length: {len(PAYLOAD)}\r\nConnection: close\r\n\r\n'.encode())
            writer.write(PAYLOAD[:1024])
            await writer.drain()
            if path == b'/primary':
                await asyncio.sleep(6)
            writer.write(PAYLOAD[1024:])
            await writer.drain()
        except (ConnectionError, asyncio.IncompleteReadError, asyncio.CancelledError):
            pass
        finally:
            writer.close()
            await writer.wait_closed()
            handlers.discard(task)

    server = await asyncio.start_server(serve, '127.0.0.1', 0)
    port = server.sockets[0].getsockname()[1]
    started = time.perf_counter()
    try:
        async with httpx.AsyncClient(trust_env=False, timeout=15.0) as client:
            result = await download([f'http://127.0.0.1:{port}/primary', f'http://127.0.0.1:{port}/backup'], client)
        assert result == PAYLOAD, 'Downloaded content differs'
        return time.perf_counter() - started, requests
    finally:
        server.close()
        await server.wait_closed()
        pending = list(handlers)
        for task in pending:
            task.cancel()
        await asyncio.gather(*pending, return_exceptions=True)


async def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', type=Path, help='Optional older video_download.py for comparison')
    args = parser.parse_args()
    if args.baseline:
        spec = importlib.util.spec_from_file_location('baseline_video_download', args.baseline)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        elapsed, requests = await measure(module.download_video)
        print(f'Baseline: {elapsed:.2f}s; requests={requests}; complete bytes verified', flush=True)
    elapsed, requests = await measure(download_video)
    assert '/backup' in requests, 'Stalled primary did not trigger fallback'
    assert elapsed < 5, f'Fallback took too long: {elapsed:.2f}s'
    print(f'Current: {elapsed:.2f}s; requests={requests}; complete bytes verified', flush=True)


if __name__ == '__main__':
    asyncio.run(main())
