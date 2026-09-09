"""Prepare temporary browser-compatible playback; never alter downloaded originals."""
import asyncio
import anyio
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import tempfile
import time

# Playback is interactive; don't let rapid episode switches exhaust CPU or temporary disk.
_slots = asyncio.Semaphore(1)
PROCESS_TIMEOUT = 180


def _tools() -> tuple[Path, Path]:
    directory = os.environ.get('HONGGUO_PLAYBACK_TOOLS_DIR')
    if not directory and getattr(sys, 'frozen', False):
        directory = str(Path(sys.executable).parent)
    if not directory:
        raise RuntimeError('缺少内置播放组件，请重新安装完整版本')
    suffix = '.exe' if sys.platform == 'win32' else ''
    tools = tuple(Path(directory) / (name + suffix) for name in ('ffmpeg', 'ffprobe'))
    if not all(path.is_absolute() and path.is_file() for path in tools):
        raise RuntimeError('缺少内置播放组件，请重新安装完整版本')
    return tools


async def _run(args: list[str], disconnected=None, timeout: float = PROCESS_TIMEOUT) -> bytes:
    options = {'creationflags': subprocess.CREATE_NO_WINDOW} if sys.platform == 'win32' else {}
    try:
        process = await asyncio.create_subprocess_exec(
            *map(str, args), stdin=asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, **options,
        )
    except OSError as exc:
        raise RuntimeError('无法启动播放兼容组件，请检查安装是否完整') from exc
    reading = asyncio.create_task(process.communicate())
    deadline = time.monotonic() + timeout
    try:
        while not reading.done():
            if disconnected is not None and await disconnected():
                raise asyncio.CancelledError()
            if time.monotonic() >= deadline:
                raise RuntimeError('视频兼容处理超时，请降低清晰度后重试')
            await asyncio.wait({reading}, timeout=0.2)
        stdout, _ = await reading
        if process.returncode:
            raise RuntimeError('视频兼容处理失败，请重试或切换剧集')
        return stdout
    finally:
        if process.returncode is None:
            try:
                process.kill()
            except ProcessLookupError:
                pass
        # Reap the encoder before deleting its temporary files, including on Windows.
        await reading


def _probe_args(ffprobe: Path, source: Path) -> list[str]:
    return [str(ffprobe), '-v', 'error', '-show_streams', '-show_format', '-show_data', '-of', 'json', str(source)]


def _video_stream(probe: dict) -> dict:
    return next((s for s in probe.get('streams', []) if s.get('codec_type') == 'video'), {})


def _compatible_video(probe: dict) -> bool:
    video = _video_stream(probe)
    return video.get('codec_name') == 'h264' and video.get('pix_fmt') == 'yuv420p'


def _compatible_audio(probe: dict) -> bool:
    return all(s.get('codec_name') == 'aac' for s in probe.get('streams', []) if s.get('codec_type') == 'audio')


def _encode_args(ffmpeg: Path, source: Path, output: Path, probe: dict) -> list[str]:
    args = [str(ffmpeg), '-hide_banner', '-loglevel', 'error', '-nostdin', '-y',
            '-threads', '2', '-i', str(source), '-map', '0:v:0', '-map', '0:a:0?',
            '-sn', '-dn', '-map_metadata', '-1']
    if _compatible_video(probe):
        args += ['-c:v', 'copy']
    else:
        args += ['-c:v', 'libx264', '-preset', 'veryfast', '-crf', '23', '-threads', '2',
                 '-vf', 'scale=trunc(iw/2)*2:trunc(ih/2)*2', '-pix_fmt', 'yuv420p']
    args += ['-tag:v', 'avc1']
    args += ['-c:a', 'copy'] if _compatible_audio(probe) else ['-c:a', 'aac', '-b:a', '128k', '-ac', '2', '-ar', '48000']
    return args + ['-movflags', '+faststart', str(output)]


async def prepare_compatible_video(data: bytes, disconnected=None) -> bytes:
    ffmpeg, ffprobe = _tools()
    # A cancelled request waiting for another episode should never start a new encoder.
    while True:
        if disconnected is not None and await disconnected():
            raise asyncio.CancelledError()
        try:
            await asyncio.wait_for(_slots.acquire(), timeout=0.2)
            break
        except asyncio.TimeoutError:
            continue
    try:
        with tempfile.TemporaryDirectory(prefix='hongguo-playback-') as directory:
            source = Path(directory) / 'source.mp4'
            output = Path(directory) / 'playback.mp4'
            source.write_bytes(data)
            try:
                probe = json.loads(await _run(_probe_args(ffprobe, source), disconnected, 30))
                if not _video_stream(probe):
                    raise RuntimeError('视频中没有可播放画面，请切换剧集')
                await _run(_encode_args(ffmpeg, source, output, probe), disconnected)
                verified = json.loads(await _run(_probe_args(ffprobe, output), disconnected, 30))
                if not _compatible_video(verified) or not _compatible_audio(verified):
                    raise RuntimeError('视频兼容处理结果无效，请重试')
                result = output.read_bytes()
            except (ValueError, OSError) as exc:
                raise RuntimeError('无法读取兼容播放视频，请重试') from exc
            if not result:
                raise RuntimeError('兼容播放视频内容为空，请重试')
            return result
    finally:
        _slots.release()


async def prepare_streaming_video(data: bytes, disconnected=None):
    """Preflight before HTTP 200, then send fragmented MP4 while FFmpeg encodes.

    Closing the browser request closes the generator and kills/reaps FFmpeg before
    deleting its source. The existing full-file path remains available for clients
    that need a complete, seekable MP4.
    """
    metadata = {}
    stream = _stream_compatible_video(data, disconnected, metadata)
    try:
        first = await anext(stream)
    except BaseException:
        await stream.aclose()
        raise

    async def chunks():
        try:
            # Prime the generator so aclose() also releases resources before its
            # first response chunk is consumed (disconnect immediately at headers).
            yield b''
            yield first
            async for chunk in stream:
                yield chunk
        finally:
            await stream.aclose()
    result = chunks()
    await anext(result)
    return result, metadata


async def _stream_compatible_video(data: bytes, disconnected=None, metadata=None):
    ffmpeg, ffprobe = _tools()
    while True:
        if disconnected is not None and await disconnected():
            raise asyncio.CancelledError()
        try:
            await asyncio.wait_for(_slots.acquire(), timeout=0.2)
            break
        except asyncio.TimeoutError:
            continue
    process = None
    reading_errors = None
    try:
        with tempfile.TemporaryDirectory(prefix='hongguo-playback-') as directory:
            source = Path(directory) / 'source.mp4'
            await asyncio.to_thread(source.write_bytes, data)
            probe = json.loads(await _run(_probe_args(ffprobe, source), disconnected, 30))
            if not _video_stream(probe):
                raise RuntimeError('视频中没有可播放画面，请切换剧集')
            args = _encode_args(ffmpeg, source, Path('unused.mp4'), probe)[:-3]
            if _compatible_video(probe):
                # ffprobe's hex dump includes an ASCII column; only decode hex.
                dump = _video_stream(probe).get('extradata', '')
                hex_data = ''.join(line.split(':', 1)[1].split('  ')[0].replace(' ', '')
                                   for line in dump.splitlines() if ':' in line)
                if not re.fullmatch('[0-9a-fA-F]{8,}', hex_data) or not hex_data.startswith('01'):
                    raise RuntimeError('无法读取视频编码参数，请重试')
                codec = 'avc1.' + hex_data[2:8]
            else:
                video = _video_stream(probe)
                width, height = int(video.get('width', 0)), int(video.get('height', 0))
                rate = video.get('r_frame_rate', '30/1').split('/')
                fps = float(rate[0]) / max(float(rate[-1]), 1)
                macroblocks = ((width + 15) // 16) * ((height + 15) // 16)
                level = 41 if macroblocks <= 8192 and macroblocks * fps <= 245760 else 42 if macroblocks <= 8704 and macroblocks * fps <= 522240 else 52
                args += ['-profile:v', 'high', '-level:v', str(level / 10)]
                codec = f'avc1.6400{level:02x}'
            # AAC-LC keeps the MediaSource codec declaration exact even for HE-AAC
            # sources. Audio conversion is small compared with video conversion.
            args += ['-c:a', 'aac', '-profile:a', 'aac_low', '-b:a', '128k', '-ar', '48000', '-ac', '2']
            has_audio = any(s.get('codec_type') == 'audio' for s in probe.get('streams', []))
            if metadata is not None:
                metadata['mime'] = f'video/mp4; codecs="{codec}' + (', mp4a.40.2' if has_audio else '') + '"'
                metadata['duration'] = str(probe.get('format', {}).get('duration', '0'))
            if not _compatible_video(probe):
                # Bound the first-fragment wait instead of buffering x264 lookahead
                # and a long GOP before the browser can decode its first frame.
                args += ['-tune', 'zerolatency', '-force_key_frames', 'expr:gte(t,n_forced*1)']
            args += ['-movflags', '+frag_keyframe+empty_moov+default_base_moof',
                     '-frag_duration', '1000000', '-flush_packets', '1', '-f', 'mp4', 'pipe:1']
            options = {'creationflags': subprocess.CREATE_NO_WINDOW} if sys.platform == 'win32' else {}
            try:
                process = await asyncio.create_subprocess_exec(
                    *args, stdin=asyncio.subprocess.DEVNULL, stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE, **options,
                )
                reading_errors = asyncio.create_task(process.stderr.read())
                try:
                    while chunk := await asyncio.wait_for(process.stdout.read(64 * 1024), PROCESS_TIMEOUT):
                        yield chunk
                    await process.wait()
                    if process.returncode:
                        raise RuntimeError('视频兼容处理失败，请重试或切换剧集')
                finally:
                    # Starlette cancels the streaming task on disconnect. Shield
                    # reaping and drain stdout so a full pipe cannot deadlock wait().
                    with anyio.CancelScope(shield=True):
                        if process.returncode is None:
                            try:
                                process.kill()
                            except ProcessLookupError:
                                pass
                        await process.stdout.read()
                        await process.wait()
                        await reading_errors
            except TimeoutError as exc:
                raise RuntimeError('视频兼容处理超时，请降低清晰度后重试') from exc
            except OSError as exc:
                raise RuntimeError('无法启动播放兼容组件，请检查安装是否完整') from exc
    except (ValueError, OSError) as exc:
        raise RuntimeError('无法读取兼容播放视频，请重试') from exc
    finally:
        _slots.release()


async def while_connected(awaitable, disconnected):
    """Cancel preparatory network work when the viewer switches/closes episodes."""
    task = asyncio.create_task(awaitable)
    try:
        while not task.done():
            if await disconnected():
                raise asyncio.CancelledError()
            await asyncio.wait({task}, timeout=0.2)
        return await task
    finally:
        if not task.done():
            task.cancel()
        await asyncio.gather(task, return_exceptions=True)
