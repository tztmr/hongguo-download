"""Compare downloads over one shared, rate-limited local HTTP link.

Uses synthetic bytes and real sockets, without upstream URLs or user media.
Eight episodes share 512 KiB/s, including any duplicate backup transfers.
"""
import argparse
import asyncio
from collections import deque
import importlib.util
from pathlib import Path
import sys
import time

import httpx

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from core.video_download import download_video

PAYLOAD = b'v' * (1024 * 1024)
CHUNK = 4096
RATE = 512 * 1024
EPISODES = 8


async def measure(download):
    connections = deque()
    handlers = set()
    requested = 0
    transferred = 0

    async def serve(reader, writer):
        nonlocal requested
        task = asyncio.current_task()
        handlers.add(task)
        state = None
        try:
            await reader.readuntil(b'\r\n\r\n')
            requested += 1
            writer.write(f'HTTP/1.1 200 OK\r\nContent-Length: {len(PAYLOAD)}\r\nConnection: close\r\n\r\n'.encode())
            state = [reader, writer, 0, asyncio.Event()]
            connections.append(state)
            await state[3].wait()
        except (ConnectionError, asyncio.IncompleteReadError, asyncio.CancelledError):
            pass
        finally:
            if state is not None and state in connections:
                connections.remove(state)
            writer.close()
            try:
                await writer.wait_closed()
            except ConnectionError:
                pass
            handlers.discard(task)

    async def pump():
        nonlocal transferred
        while True:
            if not connections:
                await asyncio.sleep(.001)
                continue
            state = connections.popleft()
            reader, writer, offset, done = state
            if reader.at_eof() or writer.is_closing():
                done.set()
                continue
            part = PAYLOAD[offset:offset + CHUNK]
            try:
                writer.write(part)
                await writer.drain()
            except ConnectionError:
                done.set()
                continue
            transferred += len(part)
            state[2] += len(part)
            if state[2] == len(PAYLOAD):
                done.set()
            else:
                connections.append(state)
            await asyncio.sleep(len(part) / RATE)

    server = await asyncio.start_server(serve, '127.0.0.1', 0)
    port = server.sockets[0].getsockname()[1]
    sender = asyncio.create_task(pump())
    started = time.perf_counter()
    try:
        async with httpx.AsyncClient(trust_env=False, timeout=60) as client:
            result = await asyncio.gather(*[
                download([f'http://127.0.0.1:{port}/primary/{i}',
                          f'http://127.0.0.1:{port}/backup/{i}'], client)
                for i in range(EPISODES)
            ])
        assert all(data == PAYLOAD for data in result), 'Content differs'
        return {'seconds': round(time.perf_counter() - started, 2),
                'requests': requested, 'transferred_bytes': transferred,
                'useful_bytes': len(PAYLOAD) * EPISODES}
    finally:
        sender.cancel()
        server.close()
        await server.wait_closed()
        pending = list(handlers)
        for task in pending:
            task.cancel()
        await asyncio.gather(sender, *pending, return_exceptions=True)


async def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', type=Path, action='append', default=[])
    args = parser.parse_args()
    for path in args.baseline:
        spec = importlib.util.spec_from_file_location('baseline_download', path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        print(f'{path.stem}: {await measure(module.download_video)}', flush=True)
    result = await measure(download_video)
    assert result['requests'] == EPISODES, 'Healthy transfers downloaded backups'
    assert result['transferred_bytes'] == result['useful_bytes'], 'Duplicate traffic'
    print(f'Fixed: {result}; all content verified', flush=True)


if __name__ == '__main__':
    asyncio.run(main())
