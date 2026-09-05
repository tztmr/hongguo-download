# 红果下载

基于现有番茄短剧纯算 API 的 macOS 桌面下载器。应用启动后会在本机拉起 FastAPI，走 `/api/duanju/download` 完成 CENC 解密，并把可播放 MP4 写到本机目录。

## 功能

- 发现页：真人剧 / 漫剧推荐流，支持翻页
- 排行榜：推荐、热播、臻果、预约、新剧、热搜、必看、收藏 8 个榜单，持续翻页到上游结束
- 今日新剧：真人剧 / 漫剧 / AI剧三类监听，并按校园、古风等标签细分；当天结果本地累计保存
- 搜索：关键词搜索后选剧、选集成片
- 下载队列：串行下载，进度条，完成后可在 Finder 中显示
- 默认保存到 `~/Downloads/红果下载/<剧名>/`
- 下载完成后可合并视频，并按集或按合并视频分离背景音乐、提取 SRT 字幕
- 使用 Google 桌面 OAuth 在系统浏览器授权 YouTube，支持可恢复上传、取消/重试和封面部分失败重试
- 下载、新剧、媒体任务和 YouTube 上传均可独立控制系统通知

## 开发

```bash
cd desktop
npm install
npx tauri dev
```

需要本机 Python 3，以及仓库根目录已安装 `requirements.txt` 依赖。

## 打包

```bash
./scripts/build-api-sidecar.sh
cd desktop
npx tauri build --config '{"bundle":{"externalBin":["binaries/hongguo-api"]}}'
```

上面的包包含本地 API，可运行榜单、新剧监听和常规下载。若要同时打包媒体处理功能，先按 `scripts/stage-media-tools.sh` 的要求提供并校验 FFmpeg/ffprobe 归档，再用 `src-tauri/tauri.release.conf.json` 构建完整 release 包。

完整发布必须使用仓库根目录的 `scripts/build-release.sh`。它会在任何构建写入前检查 FFmpeg/ffprobe 归档、SHA-256，以及五个 AI 组件的 HTTPS 发布地址、版本和校验值；缺一项就退出。成功构建后会自动调用 `scripts/verify-release.sh` 检查 ARM64 架构、可执行权限、最小 PATH 健康探针、许可证、凭证/模型泄漏、签名和 DMG。

AI 下载地址在构建预检、清单生成和安装包验证时共用 `scripts/release_urls.py`：拒绝 `.invalid`、`.test`、示例域名、本机/私有 IP、缺失主机、非法端口和带账号密码的地址。校验在清单写入或构建启动前执行，不会用无效地址覆盖原有清单。这是离线配置检查；真实链接能否下载、下载内容是否匹配 SHA-256，仍需在上传后验证。

2026-09-05 核对的五个本地 AI 压缩包及其 SHA-256、下载体积、解压文件体积已记录在 `dist/release-upload-inventory.json`，对应环境变量已整理到 `dist/release.env`。先把清单中的五个文件按原名上传到同一个真实 HTTPS 目录，再在仓库根目录设置 `HONGGUO_AI_BASE_URL`、执行 `source dist/release.env` 和 `./scripts/build-release.sh`。未设置下载目录时配置会报错退出。已有 0.1.0 安装包仍含占位地址，需在真实地址确定后重建。

## AI 组件

基础安装包不包含 PyTorch、Demucs、Whisper 或模型权重。第一次点击“分离背景音乐”或“提取字幕”时，应用会列出下载体积与安装后占用并要求确认。组件从清单中的 HTTPS 地址下载，经过 SHA-256、自检和原子发布后存放在应用数据目录；可在设置中删除或重新下载。

发布的模型归档必须直接展开到组件根目录。`demucs-htdemucs` 必须包含 `htdemucs.yaml` 及其 `models` 列表引用的全部 `<signature>[-checksum].th`；`demucs-htdemucs_ft` 对应 `htdemucs_ft.yaml` 及四个权重；Whisper 归档分别必须包含 `small.pt` 或 `medium.pt`。任何缺失、空文件或符号链接都会在任务启动前失败，Whisper 不会用运行时联网下载来补齐不完整的发布包。

权重文件准备好后，用下面的命令生成四个可安装归档和 `release-metadata.json`；该脚本会检查 Demucs bag 引用的权重是否齐全，并把 Whisper 文件规范化为运行时要求的名称。归档上传到自有 HTTPS 发布地址后，再将元数据中的 SHA-256 和体积写入发布环境变量。

```bash
python3 scripts/build-ai-model-archives.py \
  --demucs-dir /path/to/demucs-repo \
  --whisper-small /path/to/small.pt \
  --whisper-medium /path/to/medium.pt \
  --output-dir dist/ai-model-components
```

音频分离可能残留、失真或影响对白，不能保证规避 YouTube Content ID，也不改变素材版权或平台责任。字幕识别结果需要人工校对。

## YouTube 授权与上传

先在 Google Cloud 创建“桌面应用”OAuth 客户端并启用 YouTube Data API v3，然后在设置中选择下载的 JSON。应用只把通过校验的凭证私密复制到应用配置目录；刷新令牌存入 macOS 钥匙串，不写入普通状态文件或日志。授权只请求 `youtube.upload`，登录和同意必须由用户在系统浏览器中亲自完成。

上传只接受本应用验证完成的合并视频，默认优先使用合并范围生成的去背景音乐视频。发布前必须分别确认儿童受众、合成内容披露和最终发布。Google 可能把未完成审核的 OAuth 项目上传内容强制设为私享，界面以服务器返回的实际状态为准。撤销频道授权或删除本机 OAuth 配置可在设置中完成。

自动化测试不等于真实账号验收。真实 Google 同意和一次可删除测试视频的上传，需要用户在发布包中亲自执行。
