# 红果下载 AI 组件

用于红果下载 macOS ARM64 应用的可选 AI 运行环境和模型组件。

组件压缩包放在 GitHub Releases 附件中，按需下载安装。`ai-components.json` 记录下载地址、版本、SHA-256 和占用空间；`SHA256SUMS` 可用于校验下载文件。

- AI runtime：包含 Python AI worker 及其运行依赖。
- Demucs htdemucs / htdemucs_ft：音频分离模型。
- Whisper small / medium：语音字幕模型。

应用下载后校验 SHA-256，模型不包含在基础安装包中。上游软件及模型适用各自许可证；音频分离和识别效果需人工确认。

首次发布准备中，上传完成后以 Release 页面中的附件为准。
