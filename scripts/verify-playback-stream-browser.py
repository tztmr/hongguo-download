"""Exercise the production TypeScript streaming loader against bundled FFmpeg."""
import asyncio
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from fastapi import FastAPI
from fastapi.responses import HTMLResponse, Response, StreamingResponse
from core import playback
from playwright.sync_api import sync_playwright
import uvicorn


def main():
    root = Path(__file__).resolve().parents[1]
    os.environ['HONGGUO_PLAYBACK_TOOLS_DIR'] = os.environ['HONGGUO_TEST_PLAYBACK_TOOLS']
    fixtures = {name: (root / 'tests/fixtures' / name).read_bytes()
                for name in ['playback-hevc.mp4', 'playback-avc10.mp4']}
    fixtures['playback-avc-copy.mp4'] = asyncio.run(playback.prepare_compatible_video(fixtures['playback-hevc.mp4']))
    state = {}
    release = threading.Event()
    with tempfile.TemporaryDirectory(prefix='hongguo-stream-test-') as directory:
        bundle = Path(directory) / 'playback.js'
        subprocess.run(['node', '-e', 'require(process.argv[1]).buildSync(JSON.parse(process.argv[2]))',
                        str(root / 'desktop/node_modules/esbuild'), json.dumps({
                            'entryPoints': [str(root / 'desktop/src/playback.ts')],
                            'bundle': True, 'format': 'esm', 'outfile': str(bundle),
                        })], check=True)
        app = FastAPI()

        @app.get('/')
        async def index():
            return HTMLResponse('''<meta charset="utf-8"><video muted autoplay controls></video><canvas></canvas>
            <script type="module">
            import {loadEpisodeVideo} from '/playback.js';
            const name = new URL(location.href).searchParams.get('fixture');
            window.__TAURI_INTERNALS__={invoke:async()=>`${location.origin}/video?fixture=${name}&playback_compat=true`};
            const controller=new AbortController(); window.stopVideo=()=>controller.abort();
            try {
                const session=await loadEpisodeVideo('episode','720p',controller.signal);
                const video=document.querySelector('video');session.currentTime=()=>video.currentTime;
                video.src=URL.createObjectURL(session.mediaSource);
                session.finished.then(()=>window.finished=true).catch(e=>window.failure=String(e));
            } catch(e) { window.failure=String(e); }
            </script>''')

        @app.get('/playback.js')
        async def javascript():
            return Response(bundle.read_bytes(), media_type='text/javascript')

        @app.get('/video')
        async def video(fixture: str):
            stream, metadata = await playback.prepare_streaming_video(fixtures[fixture])
            async def chunks():
                try:
                    async for part in stream:
                        yield part
                    # Keep EOF pending. A Blob-based loader cannot display frames
                    # until this gate opens, making the startup regression deterministic.
                    state['tail_waiting'] = True
                    while not release.is_set():
                        await asyncio.sleep(.01)
                    state['completed'] = True
                finally:
                    await stream.aclose()
                    state['closed'] = True
            return StreamingResponse(chunks(), media_type='video/mp4', headers={
                'X-Playback-Mime': metadata['mime'], 'X-Playback-Duration': metadata['duration'],
            })

        listener = socket.socket()
        listener.bind(('127.0.0.1', 0))
        port = listener.getsockname()[1]
        server = uvicorn.Server(uvicorn.Config(app, log_level='warning'))
        thread = threading.Thread(target=server.run, kwargs={'sockets':[listener]}, daemon=True)
        thread.start()
        try:
            deadline = time.monotonic() + 10
            while not server.started and time.monotonic() < deadline:
                time.sleep(.01)
            assert server.started
            with sync_playwright() as playwright:
                browser = playwright.chromium.launch(channel=os.environ.get('HONGGUO_TEST_BROWSER', 'msedge'), headless=True)
                try:
                    for name in fixtures:
                        release.clear(); state.clear()
                        page = browser.new_page()
                        started = time.monotonic()
                        page.goto(f'http://127.0.0.1:{port}/?fixture={name}', wait_until='domcontentloaded')
                        page.wait_for_function("window.failure || document.querySelector('video').getVideoPlaybackQuality().totalVideoFrames >= 3", timeout=30000)
                        assert page.evaluate('window.failure') is None
                        assert not state.get('completed'), 'Playback waited for the entire response'
                        first_frame = time.monotonic() - started
                        release.set()
                        page.wait_for_function('window.finished || window.failure', timeout=30000)
                        assert page.evaluate('window.failure') is None
                        result = page.evaluate('''async () => {
                            const v=document.querySelector('video');
                            v.currentTime=v.duration/2;
                            await new Promise(resolve=>v.addEventListener('seeked',resolve,{once:true}));
                            const c=document.querySelector('canvas');c.width=v.videoWidth;c.height=v.videoHeight;
                            const ctx=c.getContext('2d');ctx.drawImage(v,0,0);
                            const pixels=ctx.getImageData(0,0,c.width,c.height).data,colors=new Set();
                            for(let i=0;i<pixels.length;i+=4) colors.add(`${pixels[i]},${pixels[i+1]},${pixels[i+2]}`);
                            return {width:v.videoWidth,height:v.videoHeight,colors:colors.size,time:v.currentTime,duration:v.duration};
                        }''')
                        assert result['width'] == 128 and result['height'] == 192 and result['colors'] > 32, result
                        print(json.dumps({'fixture':name,'firstFrameSeconds':first_frame,'playedBeforeEOF':True,'browser':browser.version,**result}), flush=True)
                        page.close()
                    # Closing/switching episodes must cancel a still-streaming response.
                    release.clear(); state.clear()
                    page = browser.new_page()
                    page.goto(f'http://127.0.0.1:{port}/?fixture=playback-hevc.mp4', wait_until='domcontentloaded')
                    page.wait_for_function("document.querySelector('video').getVideoPlaybackQuality().totalVideoFrames >= 3")
                    page.evaluate('window.stopVideo()')
                    deadline = time.monotonic() + 5
                    while not state.get('closed') and time.monotonic() < deadline:
                        time.sleep(.01)
                    assert state.get('closed') and not state.get('completed'), state
                    page.close()
                    print('Streaming episode cancellation: passed', flush=True)
                finally:
                    browser.close()
        finally:
            release.set()
            server.should_exit = True
            thread.join(10)
            listener.close()


if __name__ == '__main__':
    main()
