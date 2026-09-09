"""Exercise the release uploader with Windows' legacy decoding behavior."""
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import yaml

ROOT = Path(__file__).resolve().parents[1]


class WindowsReleaseUploadTests(unittest.TestCase):
    def test_unicode_release_data_and_draft_guards(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/windows-release.yml').read_text(encoding='utf-8'))
        code = next(step['run'] for step in workflow['jobs']['installer']['steps']
                    if step.get('name') == 'Upload verified installer to release draft')
        original_directory = Path.cwd()
        with tempfile.TemporaryDirectory() as directory:
            os.chdir(directory)
            folder = Path('desktop/src-tauri/target/release/bundle/nsis')
            folder.mkdir(parents=True)
            installer = folder / '红果下载_0.2.4_x64-setup.exe'
            installer.write_bytes(b'verified installer fixture')
            digest = hashlib.sha256(installer.read_bytes()).hexdigest()
            try:
                for scenario in ('valid', 'published', 'wrong_commit', 'different_asset', 'matching_asset'):
                    with self.subTest(scenario=scenario):
                        release = {'id': 1, 'tag_name': 'v0.2.4', 'name': '红果下载',
                                   'body': '首页热度优化与批量字幕提取', 'assets': [],
                                   'draft': scenario != 'published',
                                   'target_commitish': 'wrong' if scenario == 'wrong_commit' else 'commit'}
                        if scenario in ('different_asset', 'matching_asset'):
                            release['assets'] = [{'name': 'hongguo-download_0.2.4_x64-setup.exe',
                                                  'state': 'uploaded', 'size': installer.stat().st_size,
                                                  'digest': 'sha256:' + ('0' * 64 if scenario == 'different_asset' else digest)}]

                        def output(args, **kwargs):
                            result = release if args[-1].endswith('/releases/1') else [[release]]
                            raw = json.dumps(result, ensure_ascii=False).encode('utf-8')
                            # Reproduce subprocess text=True on the Windows CI runner.
                            if kwargs.get('text') or kwargs.get('encoding'):
                                return raw.decode(kwargs.get('encoding') or 'cp1252')
                            return raw

                        with patch.dict(os.environ, {'RELEASE_TAG': 'v0.2.4', 'GH_REPO': 'test/repo', 'EXPECTED_COMMIT': 'commit'}), \
                                patch('subprocess.check_output', side_effect=output), \
                                patch('subprocess.run') as upload, contextlib.redirect_stdout(io.StringIO()):
                            if scenario in ('published', 'wrong_commit', 'different_asset'):
                                with self.assertRaises(AssertionError):
                                    exec(code, {})
                            else:
                                exec(code, {})
                            self.assertEqual(upload.call_count, 1 if scenario == 'valid' else 0)
            finally:
                os.chdir(original_directory)
