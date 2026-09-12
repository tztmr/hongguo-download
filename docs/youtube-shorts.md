# Shorts 与普通视频上传

单视频、批量上传及自动追剧设置新增「上传类型」：自动识别、Shorts、普通／中长视频。默认自动识别，旧设置与上传断点保持兼容。

YouTube 按实际视频分类，没有可由 videos.insert 强制设置的 Shorts 开关。选择 Shorts 时，原生上传前用 ffprobe 校验显示画幅（含旋转和非方形像素）及容器总时长：必须竖版或方形、最多 180 秒。普通模式拒绝符合 Shorts 条件的文件，要求横版或超过 180 秒。该选项不会自动裁剪、截断或转换用户成片。自动识别保持原行为。

依据：https://support.google.com/youtube/answer/15424877?hl=en

## 2026-09-13 单集真实测试

- 当前源码 API 在本地 1427 端口成功搜索、读取目录并下载解密《贺太太不做替身了》第 1 集，book ID `7673063992922754072`，item ID `7673068267287628825`，只下载这一集。
- 原片 1080×1920、81.641 秒、HEVC/AAC。测试转码为 H.264/AAC。
- Shorts 样片取前 58 秒，1080×1920；普通样片保留完整首集，1920×1080，居中原画面加模糊背景。
- 使用原生 `ResumableUploader` 与系统保险库现有 YouTube 授权，实际上传到当前频道，均为 private。普通样片另外通过原有 thumbnail 函数设置之前生成的 Sunburst 封面。
- Shorts 样片 video ID `yGk9iHi_7f4`；普通样片 `_DIN32Fyoc4`。远端 videos.list 确认两者均 processingStatus=succeeded、uploadStatus=processed、definition=hd、privacyStatus=private；普通样片 hasCustomThumbnail=true。
- YouTube 远端时长显示分别为 PT59S 和 PT1M22S。API 没有 Shorts 分类标记；Shorts 依据上传样片的实际画幅和时长符合条件，未将上传成功等同于后台分类实测。
- 初次以原始资料和明确的测试标题上传（后续 AI 补测见下），未将整部剧 AI 剧情描述套用于单集片段。原生上传核心与真实网络已测试，24 小时调度器仍未启用，未重打包安装版。
- 原先两套运行中的旧 API 服务在设备初始化阶段解压失败；当前源码成功完成请求，本轮未修改设备注册代码或旧二进制。

## 可重复执行的原生测试入口

`desktop/src-tauri/examples/youtube_format_smoke.rs` 仅接受私享上传，复用现有 OAuth 配置和系统保险库，不导出令牌。参数为配置目录、上传请求 JSON 路径、结果目录；相同测试标题已在频道存在时跳过，上传断点和结果保存在结果目录。显式传入 shorts / standard 时，ffmpeg、ffprobe 需在可执行文件同目录。

只读检查：参数为配置目录、`--status`、逗号分隔的视频 ID、可选结果 JSON 路径。读取状态不会发起上传。

## 同日补测：真实 AI 文案与封面

- 对真实下载的 81.641 秒源视频调用已安装的 Whisper small 转录，再区分前 58 秒 Shorts 与完整开篇片段的素材事实。
- 实际调用 DeepSeek `deepseek-v4-flash`，分别生成标题候选、描述、标签。首轮出现越过 Shorts 截止时间和语言不符的内容，因此弃用；第二轮缩小素材事实范围。上传前人工校对来电时间与晚宴时间、段落排版，并从候选中选取不同标题。
- 实际调用 Moyuu `gpt-image-2.5-sunburst` 的参考图接口，使用 `coverPrompt.ts` 中固定规则和原始海报生成新封面。结果为 2048×1152，包含原图女主、两位配角、准确作品名、9 字钩子、红底黄字 HOT、黑金 1080。没有用本地拼图替代图片模型。
- 原始模型响应、最终上传请求、固定提示词、封面和审核说明保存在下载目录 `Shorts测试/贺太太不做替身了__7673063992922754072`。密钥不写入结果或仓库。
- `examples/youtube_ai_metadata_smoke.rs` 通过系统保险库现有 OAuth 更新已存在的私享视频：先核对归属和可见性，用 etag 防止覆盖并发编辑，只写 snippet，保留原分类及语言；复用应用缩略图处理函数。回读标题、描述、标签集合和私享状态。YouTube 会排序 tags，验证不依赖原始数组顺序；更新后的回读可能短暂返回旧内容，最多进行 5 次只读核对，不重发更新。
- 本次为手动驱动的真实服务串联测试，不代表 24 小时无人值守调度已经完成。Shorts 信息流实际采用哪张图仍以 YouTube 页面为准。

最终回读证据 `ai-remote-verified.json`：两条视频 metadataVerified=true、thumbnail.succeeded=true、hasCustomThumbnail=true，保持 private 和 HD。两条视频沿用原 ID，未重复上传视频。

## 自动追剧首集引流开关

自动追剧的 YouTube 上传页新增 `firstEpisodeShorts` 草稿字段，默认 false，兼容旧草稿。开启后预览“正片成功 → 首集 Shorts → 关联正片”，同步流程、摘要、单集清理与查重演示。规划使用完整首集，竖版／方形且不超过 180 秒才上传；不符合时跳过并提示。正片和首集分别查重与恢复，首集未完成前保留源文件，失败不重传正片。同一文件已作为 Shorts 上传时不再重复。

这仍是设置预览；未接入后台首集任务、自动关联或可见性变更。相关视频需在 YouTube Studio 设置，要求高级功能权限，目标正片必须为公开或不公开列出：https://support.google.com/youtube/answer/14075157?hl=en 。
