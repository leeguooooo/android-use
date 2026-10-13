# 无障碍后端：关掉 USB 调试也能控制手机

状态：设计，未实现。

## 要解决的问题

现在的链路是 `adb forward` → `app_process` 以 shell 身份跑 `Runner`，再通过 UiAutomation 读屏幕和注入输入（`src/backend.rs` 的 `AndroidBackend::connect`）。前提是开发者选项和 USB 调试都开着。任何 App 不需要权限就能读到 `Settings.Global.ADB_ENABLED` 和 `DEVELOPMENT_SETTINGS_ENABLED`，强风控 App 会据此标记账号。

只换掉 u2 jar、自己写一个 runner 解决不了这个问题。App 查的是 ADB 开关，不管 runner 是谁写的。

新方案是在手机上装一个 android-use 自己的 App，用 `AccessibilityService` 读屏幕、做操作，通过网络连回电脑。开发者选项可以全关。

这个方案是降低信号，不是消除信号。App 能用 `AccessibilityManager.getEnabledAccessibilityServiceList` 列出已启用的无障碍服务，风控 SDK 也会查。密码管理器、读屏软件同样用无障碍服务，单凭这一点封号的误伤面大，所以风险比开 USB 调试低，但不是零。README 和 SKILL 不能写"不会被封"。

## 两种后端并存

| | ADB 后端（现有） | 无障碍后端（新增） |
|---|---|---|
| 手机上装东西 | 不装 | 装一个 APK，开启无障碍权限 |
| 开发者选项 | 必须开 | 可以关 |
| 适用 | 测试、自家 App、不查调试状态的 App | 有风控的 App |

同一台手机同一时间只跑一种。A155 上实测，UiAutomation runner 在运行时，写入的无障碍服务设置会被清掉，或者服务启用了也不绑定；runner 退出后恢复正常。ADB 后端运行期间，`/data/local/tmp` 里的 runner jar 对普通 App 可见；断开时 `close()` 会删除它，token 文件在 runner 就绪后立即删除。`phone_status` 要报告当前是哪种后端。

## 改动范围

`shared/control` 不动。`DeviceBackend` 的三个方法和 `Dispatch` 的 8 个变体都能用无障碍 API 实现，不需要改 trait。如果实现中发现 trait 不够，按 AGENTS.md 先在 iphone-use 上游改、验证，再同步过来，并更新 `shared/manifest.json`。

宿主机这边新增一个后端类型（暂名 `AccessibilityBackend`），或者让 `AndroidBackend` 多一种连接方式。手机端沿用 `Runner` 的 JSON-RPC 方法名和返回结构，`read()` / `dispatch()` 的解析逻辑不用改。

传输层要换。现在 `AndroidBackend::rpc()` 是宿主机 `reqwest.post()` 到手机；新后端是手机主动连上来（见"连接与配对"），宿主机只能在这条长连接上发请求帧、按请求 id 对应响应帧。`rpc()` 要多一个传输实现，保留现有的超时和语义：只读请求失败报 `not_sent`，变更请求没收到确认报 `unknown`，都不重放。

`native_typing()` 必须返回 true。这时 `shared/control` 把 `Type` 直接翻成 `Dispatch::NativeType`（`lib.rs:650`），不会再产生 `Dispatch::SetText`，所以 `setText`、`getText`、`count` 和 u2 的选择器掩码格式都不用在手机端实现。注意 `AndroidBackend::from_endpoint` 会把 `adapter_token` 留成 `None`，那样 `read()` 会走 u2 的 `dumpWindowHierarchy` 分支，新后端不能直接复用这个构造函数。

方法参数沿用 u2 的两个约定，手机端要照着解释：`click` 带第三个参数时是长按毫秒数；`swipe` 最后一个参数是步数，宿主机传的是 `(ms / 5).clamp(1, 2000)`，每步 5 ms。

现有实现里只有两处直接调 ADB：`Dispatch::LaunchApp`（`cmd package resolve-activity` + `am start`）和 `screenshot()`（`screencap`）。手机端服务为这两项补 RPC 方法：`launchApp`、`screenshot`。

### 设备标识

现在 `Screen.device`、每台手机的进程锁、持久化的 MCP 授权（`main.rs` 的 `device_token`）都以 ADB serial 为键。新后端没有 serial。配对时由电脑生成一个设备 id，App 存在私有存储里，每次握手发上来，宿主机用它代替 serial。手机重启、App 重连后 id 不变，授权保留；App 卸载重装需要重新配对。

## 每项能力的实现

| 宿主机调用 | 手机端实现 | 最低 API |
|---|---|---|
| `read()` → `screen` | 搬 `Runner.screen()` / `walk()`，根节点改成服务自己的 `getRootInActiveWindow()` 和 `getWindows()` | 21 |
| 屏幕宽高 | `WindowManager.getCurrentWindowMetrics()`（30+），老版本用 `Display.getRealSize` | 26 |
| `Tap` / `Longpress` / `Swipe` | `dispatchGesture` + `GestureDescription`，等 `onCompleted` 回调才返回 `true`；`onCancelled` 或超时返回未确认，宿主机按 `unknown` 处理，不重放 | 24 |
| `Back` / `Home` | `performGlobalAction(GLOBAL_ACTION_BACK / HOME)` | 16 |
| `NativeType` → `typeText` | 直接搬 `Runner.typeText()`：已经是 `ACTION_SET_TEXT` 加 400 ms 只读校验，换掉 `focusedInput()` 里的 `automation` 就行 | 21 |
| `LaunchApp` | 见下一节 | 26 |
| `screenshot()` | `AccessibilityService.takeScreenshot`，转 PNG | 30 |

服务配置（`accessibility-service` XML）要声明 `canRetrieveWindowContent`、`canPerformGestures`（不声明时 `dispatchGesture` 直接返回 false）、`canTakeScreenshot`（30+），flags 加 `flagRetrieveInteractiveWindows`。

不需要自带输入法。`typeText` 现在就是 `ACTION_SET_TEXT`，不走按键事件，挪到无障碍服务里行为一致。现在就输不进去的字段（自绘控件、拿不到可编辑节点的 WebView），新后端同样输不进去。

但 `ACTION_SET_TEXT` 对目标 App 是可见的：检测 App 在自己的输入框上收到了这次无障碍操作，文字一次整段写入，没有输入法调用和按键事件；真人用键盘输入是逐字 `commitText` 加按键事件。功能上不需要输入法，要降低这项信号就得自带输入法，逐字提交。放在第二阶段。

Android 8–10（API 26–29）没有 `takeScreenshot`。第一阶段在这些版本上让 `screenshot()` 返回 `screenshot_unavailable`，`phone_capabilities` 如实报告。录屏授权（MediaProjection）每次都要弹窗确认，以后再看要不要做。

### 启动 App

Android 10 起，后台进程启动 Activity 受限。无障碍服务正在被系统绑定时是否豁免，要在 A155 上实测。

- 能豁免：`PackageManager.getLaunchIntentForPackage(pkg)` 加 `FLAG_ACTIVITY_NEW_TASK`，`startActivity`。Android 11+ 要在 manifest 里声明 `<queries>`，或者申请 `QUERY_ALL_PACKAGES`。后者上架 Play 会被审，我们走 GitHub Release 不受影响，但也会多一个敏感权限，优先只用 `<queries>` 里的 launcher intent 过滤。
- 不能豁免：退回 `Home` + 在桌面节点树里按名字找图标点。宿主机已有的 `wait_for` 可以确认是否进入目标 App。

成功判定和现在一样：返回 `true` 只代表请求发出，宿主机靠后续 `wait_for` 确认。

## 连接与配对

现在靠 `adb forward` 把手机回环端口映射到电脑，token 用 `adb push` 写到 `/data/local/tmp`。没了 ADB，这两件事都要换。

**谁连谁**：手机主动连电脑。电脑在 `android-use connect --accessibility` 时在局域网监听一个端口；手机端 App 里输入或扫码得到电脑地址后连过去，建一条长连接（WebSocket），宿主机的 RPC 走这条连接。好处是手机上不用开监听端口，App 扫本机端口也扫不到。

默认走 Wi-Fi，手机和电脑在同一局域网。没有 Wi-Fi 时可以开 USB 网络共享，电脑地址填共享网卡的 IP，这条路也不需要 USB 调试。

**配对**：

1. 电脑执行 `android-use pair`，生成 32 字节随机 token，自签 TLS 证书，终端打印二维码（内容：地址、端口、token、证书 SHA-256 指纹），同时打印一行可手抄的文本。
2. 手机 App 扫码或粘贴，保存到 App 私有存储。
3. 之后每次连接都校验证书指纹，再用 token 鉴权。

**加密**：README 现在承诺控制通道只绑回环地址。新后端有一端要在局域网监听，明文 HTTP 加 bearer token 会让同一 Wi-Fi 下的人能嗅到 token 和屏幕内容（包括输入的文字）。所以必须上 TLS，证书指纹在配对时固定。现有的 `reqwest` `rustls-tls` 只覆盖客户端，服务端要新增 `tokio-rustls`（或 `axum-server` 的 rustls 特性）、axum 的 `ws` 特性和生成自签证书的 `rcgen`。

现有的租约、owner、MCP 授权逻辑不变，它们在 `http.rs` / `mcp.rs` 那一层，跟传输无关。

## 服务存活与"掉无障碍"

无障碍方案的常见问题是权限会自己掉：App 进程被杀、崩溃次数过多、更新安装、三星电池管理冻结后台，系统都会把服务从已启用列表里移掉，之后要用户手动去设置里重新打开。

解决办法是安装时用一次 ADB 给 App 授予 `WRITE_SECURE_SETTINGS`（见"安装"）。这个授权重启后保留，只在卸载时失效。有了它，App 自己能写 `Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES` 和 `ACCESSIBILITY_ENABLED`，发现服务被移掉就自己加回去，不需要 ADB，也不需要用户操作。

自愈要靠 App 进程还活着才能触发：

- 前台服务保持连接，带常驻通知（Android 要求）。前台服务每隔几秒检查一次自己的无障碍服务是否在已启用列表里，不在就写回去。
- 开机广播（`BOOT_COMPLETED`）拉起前台服务，重启后自动恢复。
- 用户在设置里主动关掉无障碍时也会被写回去。App 里要有一个"停止并关闭无障碍"的开关，关掉后不再自愈。

宿主机仍要区分几种不可用的情况，都在 `phone_status` 里报 `drivable=false` 并给出提示，不能表现成超时：

| 情况 | 提示 |
|---|---|
| 长连接断开 | 打开手机上的 android-use App |
| 连上了，服务未启用，且没有 `WRITE_SECURE_SETTINGS` | 设置 → 无障碍 → 开启 android-use，或用 ADB 重新授权一次 |
| 连上了，但最近被系统冻结过 | 把 android-use 加入电池优化白名单 |

## 安装

APK 随 GitHub Release 发布，和 CLI 二进制放一起，附 SHA-256。签名密钥放 CI secrets，不进仓库。不发 Play。

默认路径是安装时临时开一次 ADB，装完关掉：

1. 开 USB 调试，电脑执行 `android-use install-app`。它依次做：`adb install`、`pm grant <包名> android.permission.WRITE_SECURE_SETTINGS`、`settings put secure enabled_accessibility_services <包名>/<服务类>`（追加到原有值后面，不覆盖其他服务）、`settings put secure accessibility_enabled 1`、加电池优化白名单（`cmd deviceidle whitelist +<包名>`）、把配对信息推给 App。
2. `install-app` 读回设置确认服务已启用，然后提示用户关掉开发者选项。
3. 之后运行期间全程不用 ADB。

通过 ADB 写设置开启无障碍，不经过设置界面，`adb install` 装的 App 也不会触发 Android 13+ 的"受限设置"拦截。2026-10-13 在 A155（Android 16）上用 stub 服务验证过；`am force-stop` 会把服务移出已启用列表，带 `WRITE_SECURE_SETTINGS` 的 App 能自己加回去并重新绑定。见 `docs/measurements/README.md` 的 Signal probe 一节。

没有电脑或不愿意开 ADB 的用户走手动路径：浏览器下载 APK 安装 → 应用信息 → 右上角菜单 → 允许受限设置 → 设置 → 无障碍 → 开启 android-use → 加电池优化白名单 → App 内扫 `android-use pair` 的二维码。这条路径没有 `WRITE_SECURE_SETTINGS`，掉了无障碍只能手动重开。

被授予 `WRITE_SECURE_SETTINGS` 本身也是一个信号：能看到 android-use 包的 App 可以用 `PackageManager.checkPermission` 查到。比开着 ADB 弱得多，但同样不是零。

包名和服务名不要带 `automation`、`auto`、`bot`、`script` 一类字样，避免被按名字匹配。

## 和现有文档的冲突

README 现在写着"不需要安装 Android App""不包含手机端常驻服务"。实现后要改成按后端分别说明。这次不改 README。

## 第一阶段范围

做：

- APK：无障碍服务 + 前台服务（含无障碍自愈和开机自启）+ WebSocket 客户端 + 配对界面
- RPC：`screen`、`typeText`、`click`、`swipe`、`pressKey`、`launchApp`、`screenshot`（30+）
- 宿主机：`install-app`、`pair` 命令，TLS 监听，长连接上的 RPC 传输，用设备 id 代替 serial，`phone_status` 的不可用提示
- CI：构建并签名 APK，挂到 Release

不做：

- Android 10 及以下的截图
- 自带输入法
- 点击位置、时机随机化（两种后端都需要，单独做）

## 验收

都在 Samsung SM-A155M（Android 16）上做，结果记到 `docs/measurements/`。

1. **开发者选项整个关掉**，不只是关 USB 调试。用一个只读 App 确认 `Settings.Global.ADB_ENABLED=0` 且 `DEVELOPMENT_SETTINGS_ENABLED=0`。
2. 在这个状态下跑现有的蓝牙设置 10 轮任务（`android-settings-bluetooth.json` 那组），和 ADB 后端的数据放在一起对比。
3. 跑现有 8 项输入检查（原生输入框和离线 WebView）。
4. 启动 App：确认后台启动是否被豁免，记录走的是哪条路径。
5. 两种后端切换：先连 ADB 后端再连无障碍后端，确认 UiAutomation 是否会暂停无障碍服务，以及 `phone_status` 是否报告正确。
6. 关屏、锁屏、杀后台、重启手机后，`phone_status` 给出的提示是否对应实际原因。
7. 自愈：开发者选项关闭状态下，分别用设置界面手动关掉无障碍、强行停止 App、重启手机，记录服务多久回到已启用、宿主机多久恢复 `drivable=true`。
8. `install-app` 写设置开启无障碍，确认没有被 Android 16 的受限设置拦截。
9. 用自制的检测 App（见下一节）分别在 ADB 后端和无障碍后端下各跑一遍，对比两边能被看到的信号。

### 检测 App

手上没有一个确定会因 USB 调试而拒绝运行的商业 App，所以自己做一个检测 App，模拟风控 SDK 能拿到的信号。它是普通权限的 App，不申请任何敏感权限，和第三方 App 看到的一样。放在仓库的独立目录，只用于验收，不随 Release 发布。

它采集并逐项显示：

| 信号 | 读法 |
|---|---|
| ADB / 开发者选项 | `Settings.Global.ADB_ENABLED`、`DEVELOPMENT_SETTINGS_ENABLED`、`adb_wifi_enabled` |
| 系统属性 | `sys.usb.state`、`sys.usb.config`、`init.svc.adbd`、`persist.sys.usb.config` |
| 无障碍 | `AccessibilityManager.isEnabled()`、`getEnabledAccessibilityServiceList()` 的包名列表、`Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES` |
| android-use 包 | 能否看到包（Android 11+ 的包可见性）、`checkPermission(WRITE_SECURE_SETTINGS)` |
| 本机端口 | 扫 `127.0.0.1` 常见端口和 19008，记录哪些能连上 |
| 输入事件 | 在一个测试按钮上记录每次触摸的 `deviceId`、`getDevice()` 是否为空、`source`、`toolType`、`pressure`、`size`、`touchMajor`、按下到抬起的时长、点击位置相对控件中心的偏移 |
| 文字输入 | 测试输入框记录是否收到按键事件、`InputConnection` 调用，还是整段文字被直接替换 |

每次采集结果显示在界面上，同时以 JSON 写入 App 私有目录，验收脚本读这个 JSON 存到 `docs/measurements/`。输入事件那一栏要同时录一组真人手指点击作为对照。

检测 App 只能覆盖已知的信号，商业风控 SDK 还可能用别的手段。所以它的结论只能写成"以下信号在无障碍后端下不再出现、以下信号仍然存在"，不能写成"能避开风控检测"。如果之后找到会因为 USB 调试而拒绝运行的真实 App，再补一项真实 App 验收。

## 待定

- 后台启动 Activity 的豁免情况（验收第 4 项）
- 包名和 App 显示名
- 是否需要支持一台电脑同时配对多台手机（现在的 `connect` 一次只连一台）
