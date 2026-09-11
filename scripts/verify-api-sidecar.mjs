import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export function verifySourceFiles(root, manifest) {
  if (manifest.schema !== 1 || !manifest.source_files) {
    throw new Error('API 未包含源码版本信息，请重新构建 API');
  }
  const paths = ['main.py', 'requirements.txt', 'requirements-build.txt'];
  function visit(directory) {
    for (const entry of readdirSync(join(root, directory), { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) visit(path);
      else if (entry.isFile() && entry.name.endsWith('.py')) paths.push(path);
    }
  }
  visit('core');
  visit('endpoints');
  const expected = Object.fromEntries(paths.map(path => [
    path.split('\\').join('/'), createHash('sha256').update(readFileSync(join(root, path))).digest('hex'),
  ]));
  const changed = [...new Set([...Object.keys(expected), ...Object.keys(manifest.source_files)])]
    .filter(path => expected[path] !== manifest.source_files[path]);
  if (changed.length) throw new Error(`API 与当前源码不一致：${changed.join(', ')}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  const target = process.env.TAURI_ENV_TARGET_TRIPLE || ({
    'darwin-arm64': 'aarch64-apple-darwin',
    'win32-x64': 'x86_64-pc-windows-msvc',
  })[`${process.platform}-${process.arch}`];
  try {
    if (!target && !process.argv[2]) throw new Error('请指定需要验证的 API 程序路径');
    const binary = process.argv[2] || join(root, 'desktop/src-tauri/binaries',
      `hongguo-api-${target}${target?.includes('windows') ? '.exe' : ''}`);
    const manifest = JSON.parse(execFileSync(binary, ['--build-info'], {
      encoding: 'utf8', timeout: 30000, maxBuffer: 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'],
    }));
    verifySourceFiles(root, manifest);
    console.log(`API 源码一致性检查通过：${relative(root, binary)}`);
  } catch (error) {
    console.error(`拒绝打包旧 API：${error.message}\n请先运行 scripts/build-api-sidecar.sh（macOS）或 scripts/build-api-sidecar-windows.ps1（Windows）。`);
    process.exitCode = 1;
  }
}
