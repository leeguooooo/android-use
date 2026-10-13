# android-use

Let coding agents control a real Android phone: read the accessible screen, tap, type Unicode, scroll, and execute known steps in one batch. Runs through authorized ADB on macOS, Linux, and Windows. No Android app installation, root, Python, or Java runtime is required by users.

```sh
curl -fsSL https://raw.githubusercontent.com/leeguooooo/android-use/main/install.sh | sh
android-use doctor
android-use connect
android-use elements
```

Install [Android platform-tools](https://developer.android.com/tools/releases/platform-tools), enable USB debugging, and authorize this computer on the unlocked phone. If several phones are attached, use `android-use connect --serial DEVICE_SERIAL`. For a foreground service, use `android-use serve --serial DEVICE_SERIAL`.

On Windows x64:

```powershell
irm https://raw.githubusercontent.com/leeguooooo/android-use/main/install.ps1 | iex
```

Open a new terminal after installation. macOS Intel/Apple Silicon and Linux x64 binaries ship as GitHub Release archives with SHA-256 checksums. `android-use upgrade --check` checks the latest independent release; `android-use upgrade` installs it.

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

`release` gives up the caller's lease; `disconnect` shuts down that connection and removes its runner and ADB forward. Reconnecting restarts an unhealthy connection after its owner releases it. Each phone has a process lock and each caller has an expiring lease. A different active owner cannot be displaced.

## Input and results

Input goes to the focused editable field. Normal text inserts at the current selection; `clear:true` replaces the field. Chinese, Japanese, emoji and multiline text stay intact without touching the computer's clipboard. Password replacement is redacted; password append is refused. WebView input needs an editable accessibility node. Android 8/API 26 or newer is supported by the adapter; real hardware acceptance currently covers Samsung SM-A155M, Android 16, over USB.

An acknowledged gesture means it was sent. Verify its postcondition with a fresh screen or `wait_for`. `unknown` and `retry_safe:false` forbid automatic replay. A batch stops at the first failed step and distinguishes completed steps from applied mutations. Snapshot-bound taps reject a changed screen before sending input.

Protected surfaces can be blank; inaccessible/custom-rendered fields may need visual navigation and may refuse text. This release focuses on basic phone control. Streaming, on-phone persistent services and saved-flow agents are not included.

## Development

The Rust shared control runtime and snapshot/progress helpers reuse iphone-use source; `shared/manifest.json` records the source revision and exact hashes. The device adapter is Java source compiled into a small embedded DEX; users do not build it. The UIAutomator library is downloaded separately from its pinned upstream release and verified before use. Both host and device control listeners bind to loopback and require per-session authorization.

Run `~/.local/bin/heavy-cargo test --workspace` when available, otherwise `cargo test --workspace`. Check installers with `sh scripts/test-install.sh`; check synchronized source with `python3 scripts/verify-shared.py`. Developer adapter builds require Android SDK 36 and build tools 36.0.0. CI rebuilds the adapter and verifies its source/DEX manifest before releasing binaries. See [third-party notices](THIRD-PARTY-NOTICES.md).

License: [PolyForm Shield 1.0.0](LICENSE), matching the shared iphone-use code. Third-party UIAutomator components retain their upstream licenses.
