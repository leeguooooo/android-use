# android-use — Fast Rust Android automation for AI agents

[简体中文](README.zh-CN.md) · [GitHub Releases](https://github.com/leeguooooo/android-use/releases) · [Measured performance](#measured-performance) · [Anti-detection signals](#anti-detection-fewer-debugging-signals)

**Read the screen once. Batch the next steps. Control Android with USB debugging off.**

android-use is a Rust Android automation CLI and MCP server for AI agents, including Claude Code, Codex and other MCP clients. It reads native and accessible WebView screens, taps, types Unicode, scrolls and verifies postconditions. Compact text and screen deltas reduce repeated output; batches let an agent send known actions and semantic waits in one tool call. Screenshots are available when the accessible tree is insufficient.

Two backends use the same CLI/MCP interface: the released ADB backend and an ARM64 accessibility companion in development preview. The companion connects over paired Wi-Fi TLS and works with developer options and USB debugging disabled after setup. It reduces observable debugging signals; accessibility services and input actions remain observable.

## Measured performance

Samsung SM-A155M, Android 16; fixed scripts without a model, measured on 2026-10-13. These are local development measurements unless marked as a release archive.

| Measurement | Result | Conditions and source |
|---|---:|---|
| Accessible screen read | **55.07 ms median**, 12 samples | Local native ADB adapter, USB; [raw timings](docs/measurements/bench-results.json) |
| Counter tap, exact-label wait and fresh verification | **676.51 ms median**, 10/10 tasks | Synthetic fixture, USB; [raw timings](docs/measurements/bench-results.json) |
| Settings → Connections → Bluetooth | **3.58 s median**, 10/10 tasks | Accessibility preview, Wi-Fi, developer options off; [raw timings](docs/measurements/accessibility/settings-bluetooth.json) |
| Four-step Bluetooth batch through MCP | **1,625 text bytes**, 3.69 s | One v0.1.1 release-archive sample, USB; full JSON-RPC response was 5,349 bytes; [raw record](docs/measurements/release-mcp-results.json) |

Screen-read latency, complete-task latency and response size measure different things. They are not model response times, tokens or a speed comparison against another tool. Bluetooth discovery remained active, so those completed tasks correctly reported `settled:false`. Read the [hardware acceptance conditions](docs/measurements/README.md) and [development companion results](docs/measurements/accessibility/acceptance.json) before comparing figures.

## Anti-detection: fewer debugging signals

For people evaluating **Android automation without ADB**, the accessibility preview removes the need to keep USB debugging enabled during control. A normal-permission observer on the acceptance phone recorded:

| Signal | Observed with the accessibility preview |
|---|---|
| USB debugging and developer options | Both `0` while phone control still worked |
| `adbd` / USB transport | `stopped` / `mtp` |
| Phone loopback control port `19008` | Closed; the phone connects outward to the computer |
| Runner jar in `/data/local/tmp` | Absent |
| Enabled accessibility service | Still visible |

[Raw signal evidence](docs/measurements/accessibility/developer-off-signals.json) and [ADB/human input samples](docs/measurements/README.md#signal-probe-2026-10-13) describe what was actually observed. Gestures and `ACTION_SET_TEXT` can differ from human input; a settings grant can also be visible when an app can see the package. These measurements support reduced debugging exposure, not undetectability or protection against account restrictions. No commercial platform's detection system has been validated.

## Install the released ADB backend

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

Input goes to the focused editable field. Normal text inserts at the current selection; `clear:true` replaces the field. Chinese, Japanese, emoji and multiline text stay intact without touching the computer's clipboard. Password replacement is redacted; password append is refused. WebView input needs an editable accessibility node. Android 8/API 26 or newer is supported by the adapter; real hardware acceptance covers Samsung SM-A155M, Android 16, over USB for ADB and Wi-Fi for the development companion.

An acknowledged gesture means it was sent. Verify its postcondition with a fresh screen or `wait_for`. `unknown` and `retry_safe:false` forbid automatic replay. A batch stops at the first failed step and distinguishes completed steps from applied mutations. Snapshot-bound taps reject a changed screen before sending input.

Protected surfaces can be blank; inaccessible/custom-rendered fields may need visual navigation and may refuse text. This release focuses on basic phone control. The stable ADB release does not include streaming, on-phone persistent services or saved-flow agents.

## Development

The Rust shared control runtime and snapshot/progress helpers reuse iphone-use source; `shared/manifest.json` records the source revision and exact hashes. The device adapter is Java source compiled into a small embedded DEX; users do not build it. The UIAutomator library is downloaded separately from its pinned upstream release and verified before use. ADB control listeners bind to loopback and require per-session authorization. The companion uses a separately paired TLS listener on the computer.

Run `~/.local/bin/heavy-cargo test --workspace` when available, otherwise `cargo test --workspace`. Check installers with `sh scripts/test-install.sh`; check synchronized source with `python3 scripts/verify-shared.py`. Developer adapter builds require Android SDK 36 and build tools 36.0.0. CI rebuilds the adapter and verifies its source/DEX manifest before releasing binaries. See [third-party notices](THIRD-PARTY-NOTICES.md).

License: [PolyForm Shield 1.0.0](LICENSE), matching the shared iphone-use code. Third-party UIAutomator components retain their upstream licenses.

## Accessibility companion (development)

The new companion uses Wi-Fi TLS/WebSocket and keeps the existing CLI and MCP interface. The phone opens the connection to the paired computer; it does not listen on a phone port. Runtime control does not use ADB. Android framework calls use a small Java JNI bridge; TLS, authentication, request validation and reconnection run in Rust. The current APK supports ARM64 Android 8/API 26 or newer. Screenshots require Android 11/API 30.

From a source checkout, build the preview CLI and APK with Rust's `aarch64-linux-android` target, Android SDK/build tools 36, and NDK 28.2:

```sh
rustup target add aarch64-linux-android
cargo build --release -p android-use --locked
ANDROID_SDK_ROOT=/path/to/sdk phone/build.sh
./target/release/android-use pair --host YOUR_COMPUTER_LAN_IP
./target/release/android-use install-app --apk phone/build/android-use-phone-arm64.apk
./target/release/android-use connect --accessibility
./target/release/android-use status
```

`pair` displays secret JSON and saves a private pairing file. Manual installation can paste this JSON into the phone app and enable its accessibility service in Settings. Pair only a computer you trust: it can read the screen and send input. Keep the phone and computer on the same Wi-Fi; the default computer listener is `0.0.0.0:44323`. Use `--listen ADDRESS:PORT` to restrict the interface. The agent API remains on authenticated loopback HTTP.

The ADB installation path grants `WRITE_SECURE_SETTINGS`, preserves other enabled services, exempts the companion from battery optimization and saves the pairing. Once `status` reports `backend:accessibility` and `drivable:true`, developer options can be turned off. Stop an existing ADB connection before switching backends. The certificate and token stay private in `~/.android-use`; re-pair after uninstalling the companion or changing the computer address. This initial version uses copy/paste pairing and supports one paired phone per data directory.

A persistent notification and the phone app both offer a stop action. With the one-time settings grant, an enabled companion restores its own accessibility service when it is removed. After an explicit Android force-stop, open the app and start the saved pairing; Android prevents force-stopped apps from restarting themselves. Manual installs need accessibility re-enabled in Settings. On reboot, an enabled companion resumes its saved pairing. Disconnection never replays input.

Release CI builds the APK with a checksum. Tagged releases require `ANDROID_PHONE_KEYSTORE_BASE64`, `ANDROID_PHONE_STORE_PASS` and `ANDROID_PHONE_KEY_PASS` repository secrets; pull requests use a separate development key. Development APKs must not be mistaken for signed release acceptance. Accessibility services and their input remain observable to other apps; this backend does not promise that platforms will accept automation.


## Questions about Android MCP automation

### Can Claude Code or Codex control Android with android-use?

Yes. Configure `android-use mcp` as a stdio MCP server, or use the CLI from an agent skill. The interface is not tied to one model provider. Nine tools expose status, capabilities, elements, batches, actions, taps, text, screenshots and lease release.

### Can Android automation work without USB debugging?

The accessibility development preview controls a paired phone over Wi-Fi TLS with USB debugging and developer options disabled. The default installation uses ADB once to install, authorize and pair the companion. A manual APK/settings path is also available, but has not been hardware-tested without the settings grant. The released ADB backend still requires USB debugging.

### Is android-use faster than Appium, UIAutomator or screenshot-based agents?

There is no controlled head-to-head benchmark establishing that. The local native ADB adapter measured 55.07 ms median screen reads on one Samsung Android 16 phone. Batching known steps can reduce agent tool-call round trips; actual model latency and total task time depend on the workflow. The ADB backend itself uses a pinned UIAutomator library with a source-built adapter.

### Does anti-detection mean automation cannot be detected?

No. The tested companion removed active ADB/developer-option signals, phone control ports and runner artifacts while allowing control. Enabled accessibility services and automation input remain observable. This is a measured reduction of debugging signals, not a bypass guarantee for a particular app.

### Does reconnection repeat a tap or text input?

No. Uncertain mutations return `unknown` and are never automatically replayed. Read the screen to determine the postcondition. Five reconnects within one MCP process preserved authorization in the development companion's acceptance run.

### Which Android versions and systems are supported?

The adapters target Android 8/API 26 or newer. Companion screenshots need Android 11/API 30 or newer, and the current companion APK is ARM64. Real-device acceptance covers Samsung SM-A155M with Android 16. Host release binaries cover macOS Intel/Apple Silicon, Linux x64 and Windows x64. Check [releases](https://github.com/leeguooooo/android-use/releases) for published artifacts; a preview source feature is not necessarily in the latest binary release.
