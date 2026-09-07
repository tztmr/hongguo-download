# Windows 与 NVIDIA 显卡加速适配 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> 本次交付仅为开发文档。用户要求后续再开发，不执行本计划、不生成 Windows 安装包。

**Goal:** 为红果下载增加 Windows x64 支持，覆盖 RTX 50 系列及更早 NVIDIA 显卡；可用时使用 CUDA/NVENC，不可用时提供 CPU 路径。

**Architecture:** 保留现有 React、Tauri、Rust、Python worker 结构，隔离操作系统实现。分别检测 AI 推理、视频编码与视频解码能力；按实际硬件和运行环境选择后端，不按显卡名称直接放行。AI runtime 按平台与兼容档位管理，模型权重与 runtime 分开存储。

**Tech Stack:** React 19、Tauri 2、Rust、FFmpeg/ffprobe、PyTorch/torchaudio、Demucs、Whisper、Windows WebView2。

**Spec:** 本文“需求与支持范围”来自用户 2026-09-08 的要求：“支持 50 系列和 50 以下系列，先写好 md，后续再写”。代码基线为当前工作区 0.1.10；它仍是 macOS 实现，不能视为已支持 Windows。

## 全局约束

- 首要验收机器：Intel i7（具体代数未提供）+ RTX 5060 Ti 8GB + DDR5 32GB。
- 主目标系统：Windows 11 x64。Windows 10 x64 可做独立兼容性验证，未验证前不列入正式支持；不默认承诺 Windows 7/8、32 位或 Windows ARM。
- 同一套业务逻辑继续支持 macOS；保留 VideoToolbox/MPS 路径与现有任务数据。
- “50 以下”按 NVIDIA 较早产品代际理解，包括 RTX 40/30/20、GTX 16/10/900，以及更老型号的 CPU 降级兼容；不意味着所有旧卡都能运行同一 CUDA 包。
- 正式发布前，每种加速组合必须经过真实 Windows 显卡验证。只有编译通过或 `nvidia-smi` 正常不能标记 GPU 功能可用。
- 本轮计划不加入 AMD GPU、Intel QSV、DirectML 或多 GPU 分布式推理；这些机器先提供 CPU 路径。
- 保留下载/上传暂停继续、上传最多 5 个、模型缓存、长视频分段处理、合并防重复和音画同步校验。
- GPU 加速改变处理速度，不保证提升音乐分离质量，也不保证消除音乐版权声明。

## 需求与支持范围

### 1. 显卡兼容矩阵（开发目标，全部尚未实机验收）

| 显卡范围 | AI 分离/字幕目标 | 视频编码目标 | 发布条件 |
|---|---|---|---|
| RTX 50 系列，重点 5060 Ti 8GB | 支持 Blackwell 的 CUDA runtime | 探测成功后启用 H.264 NVENC | 正确驱动、匹配的 PyTorch 构建、实际模型推理通过 |
| RTX 40 / 30 系列 | 与现代 runtime 兼容时共用 | H.264 NVENC | 各代代表型号实测，笔记本单独记录 |
| RTX 20 / GTX 16 系列 | 现代 runtime 探测通过后启用，否则兼容 runtime | H.264 NVENC，逐型号探测 | 不因产品名称推定芯片架构或编码能力 |
| GTX 10 / GTX 900 系列 | 使用仍包含对应架构的兼容 runtime；不足时 CPU | 硬件和驱动允许时 NVENC，否则 libx264 | 不能强制升级为不含旧架构的 CUDA 包 |
| 更老 GTX、GT/MX、未知型号 | CPU 为保证路径；GPU 通过完整测试后单独列入支持 | 独立探测，不支持则软件编码 | 不承诺所有型号具有 NVENC 或可用 CUDA |
| 无 NVIDIA、驱动不可用 | CPU | libx264 | 应用正常启动，下载与上传可使用 |

NVIDIA 官方矩阵按具体芯片列出编码能力，同系列也存在差异；应以探测及矩阵为准。[NVIDIA 编解码支持矩阵](https://developer.nvidia.com/video-encode-decode-support-matrix)

PyTorch 2.7 开始提供 Blackwell 与 CUDA 12.8 支持，但不能由此推导“任意更高版本都支持所有旧卡”。当前上游把部分旧架构放在不同 CUDA 构建档位，后续版本还可能收窄范围。发布时重新核对 Windows wheel、驱动要求及其架构覆盖。[PyTorch 2.7](https://pytorch.org/blog/pytorch-2-7/)、[PyTorch 发布兼容表](https://github.com/pytorch/pytorch/blob/main/RELEASE.md)

### 2. CPU 与 GPU 分工

| 工作 | 实现方向 |
|---|---|
| 下载、解密、任务调度、音频读写 | CPU、磁盘、网络 |
| 参数兼容的智能合并 | 复制视频流；音轨校正后统一编码，不为提高 GPU 占用而强制转码 |
| 需要统一画面的合并/转码 | Windows 优先 `h264_nvenc`，失败按规则回退 `libx264` |
| 分离背景音乐 | CPU 准备音频，CUDA 执行 Demucs，分段写出 |
| 语音转写 | CUDA 执行 Whisper；CPU 回退使用 FP32 |
| 生成去背景音乐视频 | 视频流复制，只替换、编码音轨 |
| YouTube 上传 | 网络传输；与 AI 任务和 NVENC 会话数量独立 |

NVENC、CUDA 与 NVDEC 是不同能力。CUDA 不可用时不应关闭已验证可用的 NVENC；NVENC 不可用时也不应禁用 CUDA。第一阶段保持 CPU 解码与现有滤镜链，NVDEC 零拷贝优化另行评估，不能只加入 `-hwaccel cuda` 就宣称全部加速。

## 当前代码中的明确适配点

以下路径均相对仓库根目录；“新增”文件尚不存在。

| 文件 | 当前情况 / 后续责任 |
|---|---|
| `ai_worker/separate.py` | `_select_device` 仅支持 auto/cpu/mps；CUDA 选择、显存错误恢复需要新增 |
| `ai_worker/transcribe.py` | 与分离共用设备选择；需验证 CUDA、FP16 和失败回退 |
| `requirements-ai.txt` | 固定 torch/torchaudio 2.5.1；不能直接拿现有包支持 RTX 50 |
| `desktop/src-tauri/src/media/merge.rs` | VideoToolbox/libx264，以及无条件 Unix 文件接口 |
| `desktop/src-tauri/src/media/smart_merge.rs` | 编码器参数与 VideoToolbox 专属回退逻辑需要解耦 |
| `desktop/src-tauri/src/media/process_control.rs` | Unix 进程组、SIGSTOP/SIGCONT/SIGTERM，不能直接用于 Windows |
| `desktop/src-tauri/src/media/tools.rs` | Unix 权限和文件标识检查，需 Windows 等价实现 |
| `desktop/src-tauri/src/media/ai.rs` | worker 环境中写死 `/usr/bin`、`/bin`；需平台化 PATH 和可执行名 |
| `desktop/src-tauri/src/media/components.rs` | 组件平台固定为 `aarch64-apple-darwin` |
| `desktop/src-tauri/src/youtube/config.rs`、`upload.rs` | Unix 文件权限接口，需保留凭证/会话文件访问限制 |
| `desktop/src-tauri/src/youtube/thumbnail.rs` | macOS 优先 sips；Windows 要验证 FFmpeg 图像解码和压缩路径 |
| `desktop/src-tauri/Cargo.toml` | keyring 仅启用 apple-native；需按目标平台配置安全存储 |
| `scripts/build-api-sidecar.sh`、`build-ai-runtime.sh`、`stage-media-tools.sh`、`render-ai-component-manifest.sh` | 目前面向 macOS ARM64；补充 Windows 构建与清单生成 |
| 新增 `desktop/src-tauri/src/media/hardware.rs` | 能力探测、诊断结果、硬件编码器选择 |
| 新增 `ai_worker/devices.py` | 集中管理设备探测与选择，避免转写/分离各自判断 |
| 新增 `scripts/build-release-windows.ps1`、`verify-release-windows.ps1` | Windows 构建、包验证、安装升级检查 |

还需检查 `lib.rs` 中的 sidecar 生命周期、文件打开、默认目录、通知与测试夹具。不能仅修改编码器字符串后就开始打包。

## 运行环境与选择策略

### Runtime 分档

- `modern`：面向 RTX 50/40/30/20、GTX 16 中通过测试的设备；必须实际包含 Blackwell 支持。
- `legacy`：面向 GTX 10/900 等旧架构，固定经过验证的 PyTorch/torchaudio/CUDA 组合，独立维护兼容范围。
- `cpu`：无 CUDA 时使用，保证基本功能；不能要求 CPU 用户额外下载完整 CUDA 包。
- 不修改 macOS 正在使用的依赖锁。为 Windows 各档位生成独立依赖锁和归档，并保存解析后的精确版本及 SHA-256。
- 普通用户安装应用和适合显卡的 NVIDIA 驱动即可；应用 runtime 携带所需用户态库，不要求用户安装 Python 或完整 CUDA Toolkit。开发构建依赖另列。

建议扩展组件清单：`platform`、`runtimeFlavor`、`supportedComputeCapabilities`、`minimumDriverVersion`、`version`、`sha256`、`downloadBytes`、`installedBytes`。架构列表和最低驱动版本从实际打包构建及发布兼容表填写，不能按型号名称猜测。

Windows 平台标识为 `x86_64-pc-windows-msvc`。缓存键至少包含平台、组件 ID、runtime 档位与版本。升级应用沿用已验证组件；不同平台/runtime 不覆盖彼此。模型权重若文件内容完全相同，可以复用经哈希验证的缓存，但仍需对应 runtime 的模型加载测试。

### 能力检测顺序

1. 读取 GPU 型号、驱动和可用显存作为诊断线索；`nvidia-smi` 不存在时继续检测，不阻止程序启动。
2. 在候选 runtime 子进程中检查 `torch.cuda.is_available()`、compute capability、构建架构列表，并执行真实 CUDA 张量运算。
3. 以短音频运行 Demucs 和 Whisper 冒烟测试，捕获缺失算子、版本冲突与模型加载失败；架构列表仅用于预筛选，不替代执行测试。
4. 使用随包 FFmpeg 执行 H.264 NVENC 短片编码，ffprobe 检查输出；`ffmpeg -encoders` 只表示构建包含接口，不表示本机能使用。
5. 缓存探测结果；驱动、runtime、FFmpeg 或 GPU 变化时失效。设置页提供重新检测和脱敏诊断导出。

设备接口约定：`_select_device(requested)` 接收 `auto/cpu/cuda/mps`；auto 在 Windows 尝试已验证 CUDA，再 CPU，在 macOS 保持 MPS/CPU 路径。显式指定不可用设备时返回可读错误，不静默假装已使用 GPU；UI 的“自动”允许降级，并显示实际设备。

### 8GB 显存与失败策略

- 首个版本每块 GPU 同时只运行一个 AI 任务，分离和字幕共用配额；CPU 工作线程也需限制，避免多个任务把机器占满。
- 继续采用现有外层分段、模型只加载一次、边界交叉淡化和逐段落盘。Demucs 内部 segment 受模型限制，不能把外层 2 分钟直接当成模型内部 segment。
- CUDA OOM：释放当前失败张量与缓存，按该模型允许的 segment 配置重试一次；仍失败时保存诊断，并在自动模式使用 CPU 重算当前未发布段。禁止跳过该段或重复写入。
- 驱动/上下文不可恢复错误：重启 worker，从检查点恢复；不能在失效上下文中无限重试。输入损坏、磁盘满、路径权限错误不归类为 GPU 故障。
- 不统一给 Demucs 开 FP16；精度调整必须先验证音质与算子兼容。Whisper CUDA FP16 同样需要实际转写验证。
- 不承诺 8GB 能并发多个模型；32GB 系统内存不能直接当作显存使用。

## 实施任务

### Task 1：Windows 基础运行与平台隔离

**文件：** 上表的 Rust 平台相关文件、Cargo.toml、sidecar 构建脚本；新增 `scripts/build-release-windows.ps1`。

**接口：** 保持业务层 `ProcessControl`、`MediaTools` 和任务模型调用不变；在模块内部使用 `cfg(unix)` / `cfg(windows)` 分派。

- [ ] 在 Windows 运行下面的编译命令，保存 Unix API、平台依赖与外部程序缺失的实际错误。
- [ ] 按目标系统拆分 imports、依赖和辅助实现；Windows 使用 `.exe` sidecar、系统应用数据目录和系统安全凭证存储。
- [ ] 文件标识与安全打开保留防替换/重解析点要求；不能用纯路径字符串替代原有句柄检查。
- [ ] 使用 Windows Job Object 管理子进程树取消及应用退出清理；不要把进程树“终止”当成“暂停”。
- [ ] Windows 暂停采用协作检查点：AI 完成当前分段、合并完成当前可恢复步骤后进入 paused。无法直接暂停的 FFmpeg 步骤终止并丢弃该步骤临时文件，继续时重做该步骤；UI 先显示“正在暂停”，实际停止后才显示“已暂停”。
- [ ] 将 `/bin/sh`、Unix 信号、符号链接等测试夹具分为平台专用测试与公共行为测试；覆盖取消后无孤儿进程、暂停后进度不变、继续无重复输出。
- [ ] Windows/macOS 分别通过编译与相关回归后，只提交本任务的文件。

```powershell
cargo check --manifest-path desktop/src-tauri/Cargo.toml --target x86_64-pc-windows-msvc
npm test --prefix desktop
cargo test --manifest-path desktop/src-tauri/Cargo.toml media::
```

验收：Windows 上能启动 UI、本地 API、FFmpeg；中文/空格路径、通知、OAuth 回调、凭证存储、暂停继续和退出清理正常。构建机准备 MSVC C++ 工具链、Rust 和 WebView2，具体依赖按 [Tauri Windows 前置要求](https://v2.tauri.app/start/prerequisites/) 安装。

### Task 2：独立的 CUDA 与 NVENC 能力探测

**文件：** 新增 `media/hardware.rs`、`ai_worker/devices.py`、`ai_worker/tests/test_devices.py`；修改 `media/mod.rs` 注册模块。

**接口：** 探测结果分别返回 `cudaAvailable`、`nvencAvailable`、`deviceName`、`driverVersion`、`computeCapability`、`runtimeFlavor`、`failureReason`；设备序号应保留，后续模型与探针使用同一设备。

- [ ] 写选择策略用例：50 系列 + 不支持架构的 runtime 不得标记可用；CUDA 不可用但 NVENC 可用仍保留编码加速；无驱动返回 CPU；显式 CUDA 不可用返回错误。
- [ ] 执行测试确认旧实现无法满足上述行为，再实现设备探测与有界超时。
- [ ] 在候选 Python 环境运行以下 CUDA 探针，并补上 Demucs/Whisper 短音频测试。

```python
import torch
print(torch.__version__, torch.version.cuda)
print(torch.cuda.is_available())
if torch.cuda.is_available():
    print(torch.cuda.get_device_name(0), torch.cuda.get_device_capability(0))
    print(torch.cuda.get_arch_list())
    x = torch.ones((128, 128), device="cuda:0")
    assert float((x @ x).mean().cpu()) == 128.0
    torch.cuda.synchronize()
```

- [ ] 在临时目录执行 NVENC 探针，退出码为零且 ffprobe 能读取有效视频才算通过。实际应用用参数数组调用，避免拼接 shell 命令。

```powershell
ffmpeg.exe -nostdin -v error -f lavfi -i color=c=black:s=1280x720:r=30 -t 1 -an -c:v h264_nvenc -pix_fmt yuv420p probe-nvenc.mp4
ffprobe.exe -v error -show_entries stream=codec_name,width,height -of json probe-nvenc.mp4
```

- [ ] 测试超时、驱动变化缓存失效、探测进程崩溃不会导致主应用崩溃；通过后单独提交。

### Task 3：视频合并接入 NVENC

**文件：** `media/merge.rs`、`media/smart_merge.rs`、`media/hardware.rs` 及相应 Rust 测试。

**接口：** 扩展现有 `VideoEncoder`，加入 `Nvenc`；实际参数由编码器构建函数生成，不复用 VideoToolbox 专有参数。

- [ ] 先增加失败用例，断言 Windows 有可用 NVENC 时选择 `h264_nvenc`；设备不可用选择 `libx264`；视频复制路径不调用编码器探测来强制转码。
- [ ] 给 NVENC 配置 H.264、8-bit yuv420p 的兼容基线，分别验证“画质优先/均衡/小体积”的参数。不同编码器的 quality 数值不可照搬。
- [ ] 将硬件初始化错误识别从 VideoToolbox 专用逻辑中拆分，覆盖驱动不匹配、无可用编码器、会话资源耗尽；磁盘满和输入损坏不得触发无意义的软件重跑。
- [ ] 合并中途硬件失败时清理本次不兼容的中间成片，统一用软件后端重建需要编码的步骤；禁止混用不兼容的 SPS/PPS 后直接拼接。
- [ ] 运行短片与超过 2 小时、包含多集接缝的真实样本；检查起点/中间/结尾音画时间轴，沿用项目现有校验阈值；验证暂停继续和重复合并保护后提交。

### Task 4：AI CUDA 推理与 runtime 发布档位

**文件：** `ai_worker/separate.py`、`transcribe.py`、`devices.py`、`tests/test_separate_streaming.py`、`tests/test_transcribe.py`；新增 Windows 各档位依赖锁和 runtime 构建脚本。

**接口：** 延续 worker 的 JSON 请求/进度协议，增加实际使用设备及降级原因；分段检查点记录输入指纹、模型版本、采样率与已发布样本数。

- [ ] 写 mock 回归：CUDA OOM 后只重算当前段、不会丢段/重复段；无 CUDA 可用时 auto 使用 CPU；转写 CPU 模式 FP16 为 false。
- [ ] 完成 Task 2 设备接口接入，分离与转写共享 GPU 配额，保留 macOS 的 MPS 行为。
- [ ] 对 modern/legacy/cpu 逐档创建干净 Windows 环境，固定并锁定经过模型加载与推理测试的依赖组合。不要直接把当前 torch 2.5.1 锁文件当作 Blackwell 环境。
- [ ] 在实际 runtime 内校验 Demucs 模型加载、torchaudio 重采样、Whisper 转写、PyInstaller DLL 收集，记录 GPU/驱动/torch/CUDA/模型版本。
- [ ] 验证旧卡仍能选择 legacy；应用升级不会覆盖可工作的旧环境；手动选择 CPU 在有独显机器上也有效。
- [ ] 通过下面的 Python 回归及真实 GPU 样本后提交，不使用无 GPU 的 CI 结果代替显卡验收。

```powershell
python -m unittest discover -s ai_worker/tests -v
```

### Task 5：组件缓存、设置诊断、完整安装包

**文件：** `media/components.rs`、`media/ai.rs`、`youtube/thumbnail.rs`、`desktop/src/settings/MediaModelsSettings.tsx`、`desktop/src/types.ts`、构建脚本；新增 `scripts/verify-release-windows.ps1`。

**接口：** 组件清单使用本计划的 platform/runtimeFlavor/版本字段；UI 展示实际 CPU/CUDA 与视频编码后端，不以下载 runtime 成功代替硬件检测成功。

- [ ] 先增加组件选择测试：Windows 不选择 macOS 包；modern/legacy 缓存共存；同版本升级安装无需重下；损坏组件只修复受影响项。
- [ ] 配置页加入“自动/CPU/NVIDIA GPU”、实际设备、可用显存、失败原因和“重新检测”。未支持的 GPU 选项禁用并解释原因。
- [ ] Windows 封面路径使用带完整 PNG/JPEG/WebP 解码能力的图像工具，验证超大封面压到当前项目要求的 2MB 以下，保留原图。
- [ ] Windows 上传链路回归：最多 5 个、暂停继续、服务器偏移恢复、v2rayN 系统代理/显式代理行为、视频成功但封面失败时仅重试封面。测试网络功能需明确授权的测试账号和视频，默认使用 mock。
- [ ] 特别回归上传源选择：整季分离完成后新建上传默认指向去背景音乐成片；已创建任务保留原上传源；确认窗口明确显示完整源路径。
- [ ] 使用 Windows CI/构建机生成 Tauri NSIS `.exe` 安装包；资源包含 Windows API sidecar、FFmpeg/ffprobe、许可证与对应清单。macOS DMG 单独发布。
- [ ] 验证基础安装包、runtime 归档的哈希、架构、DLL 完整性和干净机器启动；签名状态如实记录，不把未签名包称为已签名。
- [ ] 验证覆盖升级后设置、下载记录、上传记录、登录状态和模型缓存仍保留；完成后更新发布说明并提交。

## 最终验收清单

所有项目目前均未执行。没有对应实机时保持未勾选，并在发布说明中缩小“已验证支持”的范围。

- [ ] Windows 11 x64 + 用户 RTX 5060 Ti 8GB / i7 / 32GB：分离、字幕、NVENC、下载和上传完整通过。
- [ ] RTX 40 与 RTX 30 各一台：现代 runtime 推理及 NVENC。
- [ ] RTX 20 或 GTX 16 代表卡：能力检测、分离与转写精度、软件回退。
- [ ] GTX 10 和 GTX 900 代表卡：兼容 runtime、低显存策略、驱动限制提示。
- [ ] 无 NVIDIA 环境：所有基本功能可用，CPU 分离/转写不会错误加载 CUDA DLL。
- [ ] CUDA 可用但 NVENC 不可用，以及 NVENC 可用但 CUDA 不可用，两个方向分别测试。
- [ ] 8GB 显存长视频：运行超过 2 小时素材，内存不随时长持续增长，无音频缺口和累计音画延迟。
- [ ] 磁盘满、路径中文/空格、外置盘断开、重启、休眠恢复、暂停时关闭应用均有明确结果且不留下损坏的正式成片。
- [ ] 升级后模型复用、跨 runtime 缓存隔离；Mac 现有分离、合并和上传回归通过。
- [ ] 每个性能结果记录输入时长、分辨率、模型、硬件、驱动、软件版本、总耗时、峰值显存；不预先承诺相对 Mac 的倍数。

## 后续接手方式

在 Windows 开发环境从 Task 1 开始，先达到基础编译和运行，再接入硬件加速。每个任务完成后记录测试结果与对应提交；不要一次性修改全部后才检查。执行时检查当前工作区已有改动，避免覆盖前面 macOS 功能和用户文件。

官方资料核对日期：2026-09-08。实际发布时重新核对 PyTorch 架构支持表、Windows 驱动和 NVIDIA NVENC 能力矩阵，并把实测组合写入发布说明。
