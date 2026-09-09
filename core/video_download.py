"""Pooled CDN downloads with a delayed backup only when the first CDN stalls."""
import asyncio
import httpx

VIDEO_UA = ('com.dragon.read/58332 (Linux; U; Android 9; zh_CN; HD1900; '
            'Build/PQ3A.190705.06091305;tt-ok/3.12.13.1)')
FIRST_BYTE_WAIT = 2.0
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
    first_byte = asyncio.Event()

    async def fetch(url: str, notify: bool = False) -> bytes:
        async with client.stream('GET', url) as response:
            response.raise_for_status()
            if response.status_code != 200:
                raise RuntimeError(f'CDN HTTP {response.status_code}')
            parts = []
            async for part in response.aiter_bytes():
                if part and notify:
                    first_byte.set()
                parts.append(part)
            data = b''.join(parts)
            if len(data) < MIN_VIDEO_BYTES:
                raise RuntimeError('CDN 视频内容过小')
            return data

    if not urls:
        raise RuntimeError('没有可用视频 CDN')
    tasks = {asyncio.create_task(fetch(urls.pop(0), True))}
    first = asyncio.create_task(first_byte.wait())
    errors = []
    try:
        async with asyncio.timeout(DOWNLOAD_TIMEOUT):
            # Fast/progressing downloads never consume a second CDN connection.
            done, _ = await asyncio.wait(tasks | {first}, timeout=FIRST_BYTE_WAIT,
                                         return_when=asyncio.FIRST_COMPLETED)
            if not done and urls:
                tasks.add(asyncio.create_task(fetch(urls.pop(0))))
            while tasks:
                done, _ = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
                for task in done:
                    tasks.remove(task)
                    try:
                        return task.result()
                    except (httpx.HTTPError, RuntimeError) as exc:
                        # Never log signed CDN URLs or token-bearing exception text.
                        errors.append(type(exc).__name__)
                if urls:
                    tasks.add(asyncio.create_task(fetch(urls.pop(0))))
    except TimeoutError as exc:
        raise RuntimeError('视频 CDN 下载超时，请重试') from exc
    finally:
        first.cancel()
        for task in tasks:
            task.cancel()
        await asyncio.gather(first, *tasks, return_exceptions=True)
    raise RuntimeError(f'所有 CDN 下载失败: {", ".join(errors)}')
