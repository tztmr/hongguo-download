"""Decode synthetic compatibility outputs in the Windows browser media stack."""
import asyncio
import base64
import json
import os
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from core.playback import prepare_compatible_video
from playwright.sync_api import sync_playwright


def main():
    root = Path(__file__).resolve().parents[1]
    os.environ['HONGGUO_PLAYBACK_TOOLS_DIR'] = os.environ['HONGGUO_TEST_PLAYBACK_TOOLS']
    # One loop: the playback semaphore is shared by the requests.
    async def prepare():
        return [(name, await prepare_compatible_video((root / 'tests' / 'fixtures' / name).read_bytes()))
                for name in ['playback-hevc.mp4', 'playback-avc10.mp4']]
    videos = asyncio.run(prepare())
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(channel='msedge', headless=True)
        page = browser.new_page()
        for name, data in videos:
            page.set_content('<video muted playsinline></video><canvas></canvas>')
            page.evaluate('''async src => {
                const v = document.querySelector('video');
                v.src = src;
                await v.play();
            }''', 'data:video/mp4;base64,' + base64.b64encode(data).decode())
            page.wait_for_function('''() => {
                const v = document.querySelector('video');
                return v.videoWidth > 0 && v.getVideoPlaybackQuality().totalVideoFrames >= 3;
            }''', timeout=30000)
            result = page.evaluate('''() => {
                const v = document.querySelector('video'), c = document.querySelector('canvas');
                c.width = v.videoWidth; c.height = v.videoHeight;
                const context = c.getContext('2d'); context.drawImage(v, 0, 0);
                const pixels = context.getImageData(0, 0, c.width, c.height).data;
                const colors = new Set();
                for (let i = 0; i < pixels.length; i += 4) colors.add(`${pixels[i]},${pixels[i+1]},${pixels[i+2]}`);
                return { width: v.videoWidth, height: v.videoHeight, frames: v.getVideoPlaybackQuality().totalVideoFrames, colors: colors.size, time: v.currentTime };
            }''')
            assert result['colors'] > 32, f'Black or blank frame: {result}'
            assert result['width'] == 128 and result['height'] == 192, result
            print(json.dumps({'fixture': name, 'browser': browser.version, **result}))
        browser.close()


if __name__ == '__main__':
    main()
