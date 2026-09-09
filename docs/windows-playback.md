# Windows 在线观看兼容处理（v0.2.6）

“有声音、进度在走、画面黑屏”可能是 WebView2 无法解码上游 HEVC 视频。检查到的真实源使用 `bytevc1 / hvc1`，音频为 AAC；v0.2.1 到 v0.2.4 的播放实现本身没有变化，不能仅凭版本顺序认定是播放器代码回归。

Windows 的在线观看请求新增 `playback_compat=true`，同一清晰度优先选 AVC，其他情况用随安装包提供的 FFmpeg 将临时播放副本转为 8 位 H.264 / AAC MP4。原始下载内容、清晰度选择与 macOS 请求不变。普通 8 位 H.264 保留视频流直接重封装；HEVC 或 10 位 AVC 才重新编码。

API 从 `HONGGUO_PLAYBACK_TOOLS_DIR` 定位打包工具。处理串行、最多两个编码线程，子进程有超时；取消请求会终止并回收进程和临时文件。v0.2.6 中，Windows 使用 `playback_stream=true`：首个片段可用后即发送 fragmented MP4，前端 MediaSource 边接收边播放，不再等待整集转码及 `response.blob()`。媒体编码参数和总时长通过响应头传递，支持缓冲区内拖动；超出 SourceBuffer 容量时仅清理已观看且超出 30 秒回看窗口的内容。缺少 MediaSource 的环境保留完整 MP4 路径。

下载与播放共用原生 AES-CTR 解密，保持原先低 64 位计数器回绕语义，解密移入线程以免阻塞 API 事件循环。CDN 客户端复用连接；主线路超过 2 秒仍无首字节时才竞争备用线路，正常下载不重复请求。关闭播放器会取消准备中的网络请求和编码进程。

测试包含人工生成的 HEVC / 10 位 AVC 图案与音调（tests/fixtures），覆盖路由隔离、普通下载原样返回、取消回收及实际帧解码。Windows 发布流程使用打包版本的 FFmpeg 转换，再在 Edge 中读取视频帧数和 Canvas 像素，拒绝纯黑或空白画面。Edge 验证不能代替用户具体驱动和 WebView2 版本的实机回归。

`verify-playback-stream-browser.py` 使用生产 TypeScript 播放加载器，在服务器延迟结束响应时验证首帧确实已显示，并检查 HEVC、10 位 AVC 和普通 AVC 的画面、拖动及关闭取消。测试不承诺外网下载速度；首次播放仍需要上游加密 MP4 下载和解密完成。
