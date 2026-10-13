# android-use：面向 AI Agent 的 Rust Android 自动化工具

[English](README.md) · [下载已发布版本](https://github.com/leeguooooo/android-use/releases) · [实测记录](docs/measurements/README.md)

**读一次屏幕，一批完成后续操作；关闭 USB 调试，也能控制 Android。**

android-use 提供 CLI 和 MCP 接口，供 Claude Code、Codex 和其他 AI Agent 读取屏幕、点击、输入、滑动、启动应用和检查结果。Rust 处理控制协议与连接，无障碍后端通过 JNI 调用 Android API。

已发布的 ADB 后端和开发预览版无障碍后端使用同一套接口。无障碍后端走 Wi-Fi TLS，安装授权后可以关闭开发者选项和 USB 调试。当前 APK 支持 ARM64；功能已通过本地真机验收，尚未正式发布。

## 性能与速度

已知步骤可以和等待条件合并成一次调用，减少 Agent 反复读屏、逐步发指令的往返。接口返回可访问节点和紧凑文本，支持增量变化；需要看画面时再取截图。

2026-10-13，Samsung SM-A155M、Android 16，固定脚本、不含模型调用：

| 测量 | 结果 | 条件与证据 |
|---|---:|---|
| 可访问屏幕读取 | 中位数 **55.07 ms**，12 个样本 | 本地 ADB 适配器、USB，[原始数据](docs/measurements/bench-results.json) |
| 点击计数器、等待精确标签、独立检查 | 中位数 **676.51 ms**，10/10 通过 | 合成测试 App、USB，[原始数据](docs/measurements/bench-results.json) |
| 设置 → 连接 → 蓝牙 | 中位数 **3.58 秒**，10/10 通过 | 无障碍预览版、Wi-Fi、开发者选项关闭，[原始数据](docs/measurements/accessibility/settings-bluetooth.json) |
| MCP 四步蓝牙导航返回的文本 | **1,625 字节** | v0.1.1 发布归档单次样本；完整 JSON-RPC 为 5,349 字节，[原始记录](docs/measurements/release-mcp-results.json) |

读屏耗时、任务耗时和返回字节数分别衡量不同内容。这里没有模型耗时或 token 测量，也没有与竞品的受控对照，不能据此宣传“比某工具快几倍”。蓝牙持续扫描时，任务已完成，但 `settled:false` 仍是正确状态。

## 安装与接入

已发布的 ADB 版本：

```sh
curl -fsSL https://raw.githubusercontent.com/leeguooooo/android-use/main/install.sh | sh
android-use doctor
android-use connect
android-use elements
```

需要 Android platform-tools，并在手机上授权 USB 调试。Windows 使用仓库的 `install.ps1`。无障碍预览版需要构建 ARM64 APK 和带对应命令的 CLI，再配对；完整步骤见 [无障碍后端安装说明](README.md#accessibility-companion-development)。上面的安装器下载已发布版本，不代表已经包含开发预览功能。

任意支持 MCP 的客户端都可以配置：

```json
{"mcpServers":{"android-use":{"command":"android-use","args":["mcp"]}}}
```

Agent 技能可通过 `npx skills add leeguooooo/android-use --skill android-use` 安装。先读 `phone_status`，确认 `drivable:true` 后操作；已知步骤尽量用一个 `phone_run_steps` 批次。

## 常见问题

### 不开 USB 调试，能自动化 Android 吗？

无障碍预览版可以。默认安装流程临时用一次 ADB 安装、授权和配对，之后通过 Wi-Fi TLS 控制。手动安装 APK 并开启无障碍的路径也存在，但未完成无设置授权场景的真机验收。已发布的 ADB 后端仍需 USB 调试。

### 比 Appium、UIAutomator 或截图式 Agent 快吗？

目前没有受控对照支持这个结论。已有读屏和任务的具体耗时，可以用于复测；合并已知步骤能减少工具调用往返，最终速度还取决于模型和任务。ADB 后端本身使用固定版本的 UIAutomator 库与自研适配器。

### 断线后会自动重发点击或输入吗？

不会。无法确认的操作返回 `unknown`，需要重新读取屏幕检查结果。无障碍预览版已验证同一个 MCP 进程连续重连 5 次并保留授权。

### 目前支持哪些设备？

适配器最低目标是 Android 8/API 26；无障碍截图要求 Android 11/API 30，首版 APK 是 ARM64。真机覆盖 Samsung SM-A155M、Android 16。重启、锁屏、深度后台冻结和其他厂商机型尚未完成验收。
