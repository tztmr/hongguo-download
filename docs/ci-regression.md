# CI 回归测试

`.github/workflows/windows-ci.yml` 在 Windows 2025 与 macOS 14 上运行 Python worker、前端和 Rust 回归。Windows 另验证播放/下载/解密、发布脚本、前端构建和 Rust 程序编译。

手动运行可传 `source_ref`，指定要测试的版本标签或提交。日志中的 `Show tested revision` 是实际检出的源码提交；工作流运行页面的提交可能只是工作流自身版本。

```sh
gh workflow run windows-ci.yml --ref main -f source_ref=v0.5.0
```

Windows 库测试需要 Common Controls manifest。仅对 `cargo test --lib` 设置下列环境变量（PowerShell），并在结束后移除；普通程序构建由 Tauri 自动链接资源，重复加入会导致 MSVC 的 duplicate resource 错误。

```powershell
$env:HONGGUO_WINDOWS_TEST_RESOURCES = "1"
try {
    cargo test --manifest-path desktop/src-tauri/Cargo.toml --lib
} finally {
    Remove-Item Env:HONGGUO_WINDOWS_TEST_RESOURCES
}
cargo build --manifest-path desktop/src-tauri/Cargo.toml --bin hongguo-desktop
```

CI 分别为测试和程序构建设置环境，资源文件按 MSVC (`resource.lib`) 或 GNU (`libresource.a`) 选择。构建脚本跟踪此环境变量，避免测试资源设置沿用到程序编译。

自动追剧测试覆盖采集补位、下载恢复、合并/AI/上传阶段推进、暂停恢复和清理；并发回归通过真实调度队列与可控 executor 验证。普通托管 CI 不验证 NVIDIA GPU 推理、真实长剧处理和真实账号上传，依赖模型或媒体工具的用例会明确标记跳过或忽略。
