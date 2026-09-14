import asyncio
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, patch

import httpx
from fastapi import FastAPI
from endpoints.duanju import router, _pick_source
from core import playback


class PlaybackRouteTests(unittest.IsolatedAsyncioTestCase):
    def source(self, codec, definition='720p'):
        return {'urls': ['https://invalid.test/video'], 'spade_a': 'test', 'codec_type': codec, 'definition': definition}

    def test_prefer_avc_only_at_same_quality_and_leave_download_selection_unchanged(self):
        hevc, avc = self.source('bytevc1'), self.source('h264')
        self.assertEqual(_pick_source([hevc, avc], '720p'), hevc)
        self.assertEqual(_pick_source([hevc, avc], '720p', True), avc)
        high = self.source('bytevc1', '1080p')
        self.assertEqual(_pick_source([avc, high], '1080p', True), high)
        self.assertEqual(_pick_source([avc, high], 'auto', True), high)

    def test_download_skips_bytevc2_before_quality_fallback(self):
        bytevc2 = [self.source('bytevc2', q) for q in ['360p', '480p', '540p', '720p']]
        hevc = self.source('bytevc1', '1080p')
        # Exact shape of episode 7678806141064711230: all lower tiers are bvc2.
        self.assertEqual(_pick_source([*bytevc2, hevc], '720p', compatible_only=True), hevc)
        self.assertEqual(_pick_source([*bytevc2, hevc], 'auto', compatible_only=True), hevc)
        avc = self.source('h264', '720p')
        self.assertEqual(_pick_source([*bytevc2, hevc, avc], '720p', compatible_only=True), avc)
        # External key clients still get the requested original codec.
        self.assertEqual(_pick_source([*bytevc2, hevc], '720p'), bytevc2[-1])
        with self.assertRaisesRegex(RuntimeError, '兼容编码视频源'):
            _pick_source(bytevc2, '720p', compatible_only=True)

    async def test_download_selects_compatible_bytes_and_reports_actual_quality(self):
        app = FastAPI(); app.include_router(router, prefix='/api')
        source = self.source('bytevc1', '1080p')
        source['urls'] = ['https://invalid.test/compatible']
        download = AsyncMock(return_value=b'encrypted')
        with patch('endpoints.duanju._fetch_video_model', AsyncMock(return_value={'sources': [self.source('bytevc2'), source]})), \
             patch('endpoints.duanju.derive_key_from_spade_a', return_value='key'), \
             patch('endpoints.duanju._download_encrypted', download), \
             patch('endpoints.duanju.decrypt_mp4', return_value=b'compatible-original-mp4'):
            async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url='http://test') as client:
                response = await client.get('/api/duanju/download?item_id=7678806141064711230&definition=720p')
        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.content, b'compatible-original-mp4')
        self.assertEqual(response.headers['x-duanju-definition'], '1080p')
        self.assertEqual(response.headers['content-length'], str(len(response.content)))
        download.assert_awaited_once_with(source['urls'], None)

    async def test_unsupported_only_sources_never_return_an_unusable_download(self):
        app = FastAPI(); app.include_router(router, prefix='/api')
        download = AsyncMock()
        with patch('endpoints.duanju._fetch_video_model', AsyncMock(return_value={'sources': [self.source('bytevc2')]})), \
             patch('endpoints.duanju._download_encrypted', download), \
             patch('endpoints.duanju.asyncio.sleep', AsyncMock()):
            async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url='http://test') as client:
                response = await client.get('/api/duanju/download?item_id=episode&definition=720p')
        self.assertEqual(response.status_code, 502)
        self.assertIn('兼容编码视频源', response.json()['msg'])
        download.assert_not_awaited()

    async def test_only_playback_converts_and_regular_download_keeps_original_bytes(self):
        app = FastAPI(); app.include_router(router, prefix='/api')
        converter = AsyncMock(return_value=b'compatible-mp4')
        with patch('endpoints.duanju._fetch_video_model', AsyncMock(return_value={'sources':[self.source('bytevc1')]})), \
             patch('endpoints.duanju.derive_key_from_spade_a', return_value='key'), \
             patch('endpoints.duanju._download_encrypted', AsyncMock(return_value=b'encrypted')), \
             patch('endpoints.duanju.decrypt_mp4', return_value=b'original-mp4'), \
             patch('endpoints.duanju.prepare_compatible_video', converter):
            async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url='http://test') as client:
                original = await client.get('/api/duanju/download?item_id=episode&definition=720p')
                self.assertEqual(original.content, b'original-mp4')
                converter.assert_not_called()
                compatible = await client.get('/api/duanju/download?item_id=episode&definition=720p&playback_compat=true')
                self.assertEqual(compatible.status_code, 200)
                self.assertEqual(compatible.content, b'compatible-mp4')
                self.assertEqual(compatible.headers['x-duanju-playback'], 'h264-aac')
                self.assertEqual(compatible.headers['cache-control'], 'no-store')
                converter.assert_awaited_once()

    async def test_failed_compatibility_returns_actionable_error_not_unplayable_video(self):
        app = FastAPI(); app.include_router(router, prefix='/api')
        with patch('endpoints.duanju._fetch_video_model', AsyncMock(return_value={'sources':[self.source('hevc')]})), \
             patch('endpoints.duanju.derive_key_from_spade_a', return_value='key'), \
             patch('endpoints.duanju._download_encrypted', AsyncMock(return_value=b'encrypted')), \
             patch('endpoints.duanju.decrypt_mp4', return_value=b'original-mp4'), \
             patch('endpoints.duanju.prepare_compatible_video', AsyncMock(side_effect=RuntimeError('缺少内置播放组件'))):
            async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url='http://test') as client:
                response = await client.get('/api/duanju/download?item_id=episode&playback_compat=true')
                self.assertEqual(response.status_code, 502)
                self.assertIn('缺少内置播放组件', response.json()['msg'])


class PlaybackProcessTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        playback._slots = asyncio.Semaphore(1)

    async def test_disconnect_kills_encoder_and_reaps_it(self):
        done = asyncio.Event()
        async def communicate():
            await done.wait()
            process.returncode = -9
            return b'', b''
        process = AsyncMock()
        process.returncode = None
        process.communicate = communicate
        def kill(): done.set()
        from unittest.mock import Mock
        process.kill = Mock(side_effect=kill)
        with patch('core.playback.asyncio.create_subprocess_exec', AsyncMock(return_value=process)):
            with self.assertRaises(asyncio.CancelledError):
                await playback._run(['ffmpeg'], AsyncMock(return_value=True))
        process.kill.assert_called_once()
        self.assertEqual(process.returncode, -9)

    async def test_close_during_preparation_cancels_network_task(self):
        closed = asyncio.Event()
        async def pending():
            try:
                await asyncio.Event().wait()
            finally:
                closed.set()
        started = asyncio.Event()
        async def disconnected():
            await started.wait()
            return True
        task = asyncio.create_task(playback.while_connected(pending(), disconnected))
        await asyncio.sleep(0)
        await asyncio.sleep(0)
        started.set()
        with self.assertRaises(asyncio.CancelledError): await task
        self.assertTrue(closed.is_set())

    async def test_missing_tools_fail_before_any_processing(self):
        with patch.dict(os.environ, {'HONGGUO_PLAYBACK_TOOLS_DIR': '/nonexistent/hongguo-tools'}):
            with self.assertRaisesRegex(RuntimeError, '缺少内置播放组件'):
                await playback.prepare_compatible_video(b'mp4')

    def test_h264_is_copied_but_hevc_or_ten_bit_avc_is_reencoded(self):
        def args(codec, pix_fmt):
            return playback._encode_args(Path('/ffmpeg'), Path('/in.mp4'), Path('/out.mp4'), {'streams':[{'codec_type':'video','codec_name':codec,'pix_fmt':pix_fmt}]})
        avc = args('h264', 'yuv420p')
        self.assertEqual(avc[avc.index('-c:v') + 1], 'copy')
        for codec, pixel in [('hevc','yuv420p'), ('h264','yuv420p10le')]:
            encoded = args(codec, pixel)
            self.assertEqual(encoded[encoded.index('-c:v')+1], 'libx264')
            self.assertIn('yuv420p', encoded)

    @unittest.skipUnless(os.environ.get('HONGGUO_TEST_PLAYBACK_TOOLS'), 'requires bundled FFmpeg/FFprobe')
    async def test_stream_is_fragmented_decodable_and_disconnect_releases_slot(self):
        tools = Path(os.environ['HONGGUO_TEST_PLAYBACK_TOOLS']).resolve()
        suffix = '.exe' if os.name == 'nt' else ''
        ffmpeg = tools / ('ffmpeg'+suffix)
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {'HONGGUO_PLAYBACK_TOOLS_DIR':str(tools)}):
            source = (Path(__file__).parent / 'fixtures' / 'playback-hevc.mp4').read_bytes()
            for data in [source, await playback.prepare_compatible_video(source)]:
                stream, metadata = await playback.prepare_streaming_video(data)
                self.assertIn('avc1.', metadata['mime'])
                self.assertGreater(float(metadata['duration']), 0)
                output = Path(directory) / 'stream.mp4'
                output.write_bytes(b''.join([part async for part in stream]))
                self.assertIn(b'moof', output.read_bytes())
                decoded = await playback._run([str(ffmpeg), '-v', 'error', '-i', str(output), '-an', '-frames:v', '2', '-f', 'framemd5', '-'])
                self.assertEqual(len([line for line in decoded.splitlines() if not line.startswith(b'#')]), 2)
            processes = []
            original_spawn = asyncio.create_subprocess_exec
            async def spawn(*args, **kwargs):
                p = await original_spawn(*args, **kwargs)
                processes.append(p)
                return p
            with patch('core.playback.asyncio.create_subprocess_exec', spawn):
                stream, _ = await playback.prepare_streaming_video(source)
                self.assertTrue(playback._slots.locked())
                await stream.aclose()
            self.assertFalse(playback._slots.locked())
            self.assertTrue(all(p.returncode is not None for p in processes))

    @unittest.skipUnless(os.environ.get('HONGGUO_TEST_PLAYBACK_TOOLS'), 'requires bundled FFmpeg/FFprobe')
    async def test_real_hevc_and_ten_bit_avc_are_compatible_and_decodable(self):
        tools = Path(os.environ['HONGGUO_TEST_PLAYBACK_TOOLS']).resolve()
        suffix = '.exe' if os.name == 'nt' else ''
        ffmpeg, ffprobe = tools / ('ffmpeg'+suffix), tools / ('ffprobe'+suffix)
        with tempfile.TemporaryDirectory(prefix='播放验证 ') as directory, patch.dict(os.environ, {'HONGGUO_PLAYBACK_TOOLS_DIR':str(tools)}):
            for fixture in ['playback-hevc.mp4', 'playback-avc10.mp4']:
                source = Path(directory) / '源视频.mp4'
                source.write_bytes((Path(__file__).parent / 'fixtures' / fixture).read_bytes())
                original = source.read_bytes()
                result = await playback.prepare_compatible_video(original)
                self.assertEqual(source.read_bytes(), original)
                output = Path(directory) / '兼容播放.mp4'; output.write_bytes(result)
                probe = json.loads(await playback._run(playback._probe_args(ffprobe, output)))
                self.assertTrue(playback._compatible_video(probe))
                self.assertTrue(playback._compatible_audio(probe))
                # Decode actual frames; valid container metadata alone is insufficient.
                decoded = await playback._run([str(ffmpeg), '-v', 'error', '-i', str(output), '-an', '-frames:v', '2', '-f', 'framemd5', '-'])
                frames = [line for line in decoded.splitlines() if not line.startswith(b'#')]
                self.assertEqual(len(frames), 2)
                self.assertLess(result.find(b'moov'), result.find(b'mdat'))

if __name__ == '__main__':
    unittest.main()
