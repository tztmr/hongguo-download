"""Prepare temporary browser-compatible playback; never alter downloaded originals."""
import asyncio
import json
import os
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
    return [str(ffprobe), '-v', 'error', '-show_streams', '-of', 'json', str(source)]


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
