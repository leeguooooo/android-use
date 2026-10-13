# android-use

Let coding agents control a real Android phone: read the accessible screen, tap, type Unicode, scroll, and execute known steps in one batch. Runs through authorized ADB on macOS, Linux, and Windows. The ADB backend needs no Android app installation, root, Python, or Java runtime. An optional accessibility companion is under development.

```sh
curl -fsSL https://raw.githubusercontent.com/leeguooooo/android-use/main/install.sh | sh
android-use doctor
android-use connect
android-use elements
```

Install [Android platform-tools](https://developer.android.com/tools/releases/platform-tools), enable USB debugging, and authorize this computer on the unlocked phone. If several phones are attached, use `android-use connect --serial DEVICE_SERIAL`. For a foreground service, use `android-use serve --serial DEVICE_SERIAL`.

On Windows 10 or newer (x64):

```powershell
irm https://raw.githubusercontent.com/leeguooooo/android-use/main/install.ps1 | iex
```

Open a new terminal after installation. macOS Intel/Apple Silicon and Linux x64 (glibc 2.35+) binaries ship as GitHub Release archives with SHA-256 checksums. `android-use upgrade --check` checks the latest independent release; `android-use upgrade` installs it.

## Agents

```sh
npx skills add leeguooooo/android-use --skill android-use
```

For Claude Code, install `android-use@leeguooooo-plugins` from the use-family marketplace. For any MCP client:

```json
{"mcpServers":{"android-use":{"command":"android-use","args":["mcp"]}}}
```

The stdio server exposes `phone_status`, `phone_capabilities`, `phone_elements`, `phone_run_steps`, `phone_actions`, `phone_tap`, `phone_type`, `phone_screenshot`, and `phone_release`. Read once, batch what is already known, and wait for an exact app or element after navigation. Mutations return an observation by default; screenshots are on demand. MCP text is compact while structured results retain geometry and flags.

```sh
android-use --owner example act '{"steps":[{"type":"launch_app","app":"com.android.settings"},{"type":"wait_for","expect":{"app":"com.android.settings"},"timeout_ms":5000}]}'
android-use --owner example elements
android-use --owner example act '{"type":"tap","element":3,"snapshot":"SNAPSHOT_FROM_READ"}'
android-use --owner example release
android-use disconnect
```

`release` gives up the caller's lease; `disconnect` shuts down that connection and removes its runner and ADB forward. Reconnecting restarts an unhealthy connection after its owner releases it. MCP authorization stays valid when the same phone reconnects. Each phone has a process lock and each caller has an expiring lease. A different active owner cannot be displaced.

## Input and results

Input goes to the focused editable field. Normal text inserts at the current selection; `clear:true` replaces the field. Chinese, Japanese, emoji and multiline text stay intact without touching the computer's clipboard. Password replacement is redacted; password append is refused. WebView input needs an editable accessibility node. Android 8/API 26 or newer is supported by the adapter; real hardware acceptance currently covers Samsung SM-A155M, Android 16, over USB.

An acknowledged gesture means it was sent. Verify its postcondition with a fresh screen or `wait_for`. `unknown` and `retry_safe:false` forbid automatic replay. A batch stops at the first failed step and distinguishes completed steps from applied mutations. Snapshot-bound taps reject a changed screen before sending input.

Protected surfaces can be blank; inaccessible/custom-rendered fields may need visual navigation and may refuse text. This release focuses on basic phone control. The stable ADB release does not include streaming, on-phone persistent services or saved-flow agents.

## Development

The Rust shared control runtime and snapshot/progress helpers reuse iphone-use source; `shared/manifest.json` records the source revision and exact hashes. The device adapter is Java source compiled into a small embedded DEX; users do not build it. The UIAutomator library is downloaded separately from its pinned upstream release and verified before use. ADB control listeners bind to loopback and require per-session authorization. The companion uses a separately paired TLS listener on the computer.

Run `~/.local/bin/heavy-cargo test --workspace` when available, otherwise `cargo test --workspace`. Check installers with `sh scripts/test-install.sh`; check synchronized source with `python3 scripts/verify-shared.py`. Developer adapter builds require Android SDK 36 and build tools 36.0.0. CI rebuilds the adapter and verifies its source/DEX manifest before releasing binaries. See [third-party notices](THIRD-PARTY-NOTICES.md).

License: [PolyForm Shield 1.0.0](LICENSE), matching the shared iphone-use code. Third-party UIAutomator components retain their upstream licenses.

## Accessibility companion (development)

The new companion uses Wi-Fi TLS/WebSocket and keeps the existing CLI and MCP interface. The phone opens the connection to the paired computer; it does not listen on a phone port. Runtime control does not use ADB. Android framework calls use a small Java JNI bridge; TLS, authentication, request validation and reconnection run in Rust. The current APK supports ARM64 Android 8/API 26 or newer. Screenshots require Android 11/API 30.

Build an APK with Rust's `aarch64-linux-android` target, Android SDK/build tools 36, and NDK 28.2:

```sh
rustup target add aarch64-linux-android
ANDROID_SDK_ROOT=/path/to/sdk phone/build.sh
android-use pair --host YOUR_COMPUTER_LAN_IP
android-use install-app --apk phone/build/android-use-phone-arm64.apk
android-use connect --accessibility
android-use status
```

`pair` displays secret JSON and saves a private pairing file. Manual installation can paste this JSON into the phone app and enable its accessibility service in Settings. Pair only a computer you trust: it can read the screen and send input. Keep the phone and computer on the same Wi-Fi; the default computer listener is `0.0.0.0:44323`. Use `--listen ADDRESS:PORT` to restrict the interface. The agent API remains on authenticated loopback HTTP.

The ADB installation path grants `WRITE_SECURE_SETTINGS`, preserves other enabled services, exempts the companion from battery optimization and saves the pairing. Once `status` reports `backend:accessibility` and `drivable:true`, developer options can be turned off. Stop an existing ADB connection before switching backends. The certificate and token stay private in `~/.android-use`; re-pair after uninstalling the companion or changing the computer address. This initial version uses copy/paste pairing and supports one paired phone per data directory.

A persistent notification and the phone app both offer a stop action. With the one-time settings grant, an enabled companion restores its own accessibility service when it is removed. After an explicit Android force-stop, open the app and start the saved pairing; Android prevents force-stopped apps from restarting themselves. Manual installs need accessibility re-enabled in Settings. On reboot, an enabled companion resumes its saved pairing. Disconnection never replays input.

Release CI builds the APK with a checksum. Tagged releases require `ANDROID_PHONE_KEYSTORE_BASE64`, `ANDROID_PHONE_STORE_PASS` and `ANDROID_PHONE_KEY_PASS` repository secrets; pull requests use a separate development key. Development APKs must not be mistaken for signed release acceptance. Accessibility services and their input remain observable to other apps; this backend does not promise that platforms will accept automation.
