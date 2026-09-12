"""Pooled CDN downloads; hedge stalled transfers, not shared-bandwidth rates."""
import asyncio
import httpx

VIDEO_UA = ('com.dragon.read/58332 (Linux; U; Android 9; zh_CN; HD1900; '
            'Build/PQ3A.190705.06091305;tt-ok/3.12.13.1)')
FIRST_BYTE_WAIT = 2.0
STALL_WAIT = 3.0
DOWNLOAD_TIMEOUT = 120.0
MIN_VIDEO_BYTES = 1024


def create_video_client() -> httpx.AsyncClient:
    return httpx.AsyncClient(
        timeout=httpx.Timeout(15.0, connect=8.0, pool=15.0), verify=False,
        follow_redirects=True, headers={'User-Agent': VIDEO_UA},
        limits=httpx.Limits(max_connections=32, max_keepalive_connections=16),
    )


async def download_video(urls: list[str], client: httpx.AsyncClient) -> bytes:
    urls = list(dict.fromkeys(urls))
    loop = asyncio.get_running_loop()
    progress = {}

    async def fetch(url: str) -> bytes:
        async with client.stream('GET', url) as response:
            response.raise_for_status()
            if response.status_code != 200:
                raise RuntimeError(f'CDN HTTP {response.status_code}')
            parts = []
            async for part in response.aiter_bytes():
                if part:
                    state = progress[asyncio.current_task()]
                    state[1] = loop.time()
                    state[2] += len(part)
                parts.append(part)
            data = b''.join(parts)
            if len(data) < MIN_VIDEO_BYTES:
                raise RuntimeError('CDN 视频内容过小')
            return data

    if not urls:
        raise RuntimeError('没有可用视频 CDN')
    tasks = set()

    def start_next():
        task = asyncio.create_task(fetch(urls.pop(0)))
        now = loop.time()
        progress[task] = [now, now, 0]
        tasks.add(task)

    start_next()
    errors = []
    try:
        async with asyncio.timeout(DOWNLOAD_TIMEOUT):
            while tasks:
                # Watch the whole transfer, not just the first byte. Keep at most
                # two CDN requests per episode, and retain a progressing primary
                # until a complete, valid alternative has actually arrived.
                done, _ = await asyncio.wait(
                    tasks, timeout=min(0.25, FIRST_BYTE_WAIT / 2, STALL_WAIT / 2),
                    return_when=asyncio.FIRST_COMPLETED,
                )
                for task in done:
                    tasks.remove(task)
                    progress.pop(task)
                    try:
                        return task.result()
                    except (httpx.HTTPError, RuntimeError) as exc:
                        # Never log signed CDN URLs or token-bearing exception text.
                        errors.append(type(exc).__name__)
                if not tasks and urls:
                    start_next()
                elif len(tasks) == 1 and urls:
                    started, last_byte, received = progress[next(iter(tasks))]
                    now = loop.time()
                    elapsed = now - started
                    waiting = received == 0 and elapsed >= FIRST_BYTE_WAIT
                    stalled = received > 0 and now - last_byte >= STALL_WAIT
                    # A low per-episode rate can simply mean that all episodes
                    # share the same link. Racing a second full copy then steals
                    # bandwidth from useful downloads (v0.3.1 regression).
                    # Only hedge when bytes stop arriving, as opposed to using
                    # an absolute KiB/s cutoff for a progressing transfer.
                    if waiting or stalled:
                        start_next()
    except TimeoutError as exc:
        raise RuntimeError('视频 CDN 下载超时，请重试') from exc
    finally:
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
    raise RuntimeError(f'所有 CDN 下载失败: {", ".join(errors)}')
