"""Pooled CDN downloads; hedge stalled transfers, not shared-bandwidth rates."""
import asyncio
from dataclasses import dataclass
import httpx

VIDEO_UA = ('com.dragon.read/58332 (Linux; U; Android 9; zh_CN; HD1900; '
            'Build/PQ3A.190705.06091305;tt-ok/3.12.13.1)')
FIRST_BYTE_WAIT = 2.0
STALL_WAIT = 3.0
DOWNLOAD_TIMEOUT = 120.0
MIN_VIDEO_BYTES = 1024
POOL_RETRIES = 2
POOL_RETRY_DELAY = 0.5


class VideoPoolBusy(RuntimeError):
    """Local connection pressure; callers should queue instead of changing CDNs."""

    def __init__(self):
        super().__init__('下载连接繁忙，请稍后重试')


@dataclass
class _Progress:
    started: float | None = None
    last_byte: float = 0.0
    received: int = 0


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
        state = progress[asyncio.current_task()]

        async def trace(event, _info):
            # HTTPX transport events start only after acquiring a connection.
            # Do not treat time spent queued locally as a stalled CDN. Never
            # retain/log trace info, which can contain signed URLs and headers.
            if event.endswith('.started') and state.started is None:
                state.started = loop.time()

        for attempt in range(POOL_RETRIES + 1):
            try:
                async with client.stream('GET', url, extensions={'trace': trace}) as response:
                    if state.started is None:
                        state.started = loop.time()
                    response.raise_for_status()
                    if response.status_code != 200:
                        raise RuntimeError(f'CDN HTTP {response.status_code}')
                    parts = []
                    async for part in response.aiter_bytes():
                        if part:
                            state.last_byte = loop.time()
                            state.received += len(part)
                        parts.append(part)
                    data = b''.join(parts)
                    if len(data) < MIN_VIDEO_BYTES:
                        raise RuntimeError('CDN 视频内容过小')
                    return data
            except httpx.PoolTimeout:
                # Retry the same address after allowing other streams to finish;
                # changing CDN would just add another waiter to the same pool.
                state.started = None
                if attempt == POOL_RETRIES:
                    raise VideoPoolBusy() from None
                await asyncio.sleep(POOL_RETRY_DELAY * (2 ** attempt))

    if not urls:
        raise RuntimeError('没有可用视频 CDN')
    tasks = set()

    def start_next():
        task = asyncio.create_task(fetch(urls.pop(0)))
        progress[task] = _Progress()
        tasks.add(task)

    start_next()
    errors = []
    pool_busy = False
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
                    except VideoPoolBusy:
                        pool_busy = True
                    except (httpx.HTTPError, RuntimeError) as exc:
                        # Never log signed CDN URLs or token-bearing exception text.
                        errors.append(type(exc).__name__)
                if not tasks and pool_busy:
                    raise VideoPoolBusy()
                if not tasks and urls:
                    start_next()
                elif len(tasks) == 1 and urls and not pool_busy:
                    state = progress[next(iter(tasks))]
                    if state.started is None:
                        continue
                    now = loop.time()
                    waiting = state.received == 0 and now - state.started >= FIRST_BYTE_WAIT
                    stalled = state.received > 0 and now - state.last_byte >= STALL_WAIT
                    # A low per-episode rate can simply mean that all episodes
                    # share the same link. Racing a second full copy then steals
                    # bandwidth from useful downloads (v0.3.1 regression).
                    # Only hedge when bytes stop arriving, as opposed to using
                    # an absolute KiB/s cutoff for a progressing transfer.
                    if waiting or stalled:
                        start_next()
    except TimeoutError as exc:
        if pool_busy or (tasks and all(progress[task].started is None for task in tasks)):
            raise VideoPoolBusy() from None
        raise RuntimeError('视频 CDN 下载超时，请重试') from exc
    finally:
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
    raise RuntimeError(f'所有 CDN 下载失败: {", ".join(errors)}')
