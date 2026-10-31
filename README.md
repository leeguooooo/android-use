# android-use — Fast Rust Android automation for AI agents

[简体中文](README.zh-CN.md) · [GitHub Releases](https://github.com/leeguooooo/android-use/releases) · [Measured performance](#measured-performance)

**Read the screen once. Batch the next steps. Control a real Android phone.**

android-use is a Rust Android automation CLI and MCP server for AI agents, including Claude Code, Codex and other MCP clients. It reads native and accessible WebView screens, taps, types Unicode, scrolls and verifies postconditions. Compact text and screen deltas reduce repeated output; batches let an agent send known actions and semantic waits in one tool call. Screenshots are available when the accessible tree is insufficient.

Two backends use the same CLI/MCP interface: the released ADB backend and an ARM64 accessibility companion in development preview. The companion connects to a paired computer over Wi-Fi TLS.

## Measured performance

Samsung SM-A155M, Android 16; fixed scripts without a model, measured on 2026-10-13. These are local development measurements unless marked as a release archive.

| Measurement | Result | Conditions and source |
|---|---:|---|
| Accessible screen read | **55.07 ms median**, 12 samples | Local native ADB adapter, USB; [raw timings](docs/measurements/bench-results.json) |
| Counter tap, exact-label wait and fresh verification | **676.51 ms median**, 10/10 tasks | Synthetic fixture, USB; [raw timings](docs/measurements/bench-results.json) |
| Settings → Connections → Bluetooth | **3.58 s median**, 10/10 tasks | Accessibility preview, Wi-Fi; [raw timings](docs/measurements/accessibility/settings-bluetooth.json) |
| Four-step Bluetooth batch through MCP | **1,625 text bytes**, 3.69 s | One v0.1.1 release-archive sample, USB; full JSON-RPC response was 5,349 bytes; [raw record](docs/measurements/release-mcp-results.json) |

Screen-read latency, complete-task latency and response size measure different things. They are not model response times, tokens or a speed comparison against another tool. Bluetooth discovery remained active, so those completed tasks correctly reported `settled:false`. Read the [hardware acceptance conditions](docs/measurements/README.md) and [development companion results](docs/measurements/accessibility/acceptance.json) before comparing figures.

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

`android-use act --help` lists every action and its fields, and `phone_capabilities` carries the same data as `action_schema`. A rejected action names the unknown type, the missing or unexpected field, or the batch step at fault.

When an app draws on a canvas (Unity, Flutter games), `elements` adds `canvas_rendered:true` and a `hint`. The check is a heuristic: at most one named element outside the status and navigation bars. `android-use screenshot out.png --max-width 720 --crop 0,0.5,1,0.5` returns a smaller PNG; there is no OCR.

### ADB tools

These run on the computer and need USB debugging. Without an attached ADB device, such as a phone driven only by the accessibility companion, they return `adb_required`. They were tested against a fake `adb`, not a real phone. `capabilities` lists them as `adb_only_commands`.

```sh
android-use adb recents
android-use adb force-stop com.example.app
android-use adb restart com.example.app
android-use adb logcat --tag MyTag --pid 1234 --since 1793000000 --lines 200
android-use adb pull /sdcard/Download/logs.zip ./logs.zip
android-use adb package com.example.app   # omit the name for the foreground app
android-use adb apps --all
```

`recents`, `force-stop` and `restart` first take the daemon lease, so they refuse while another caller owns the phone. `package` returns version, install times, APK paths and `running`; `label` stays null because ADB does not expose it.

## Flows

A flow is a saved, validated list of steps replayed one step per batch, so each step gets its own outcome, timing and failure capture. Flows live in `~/.android-use/flows/<category>/<slug>.json` (override with `ANDROID_USE_FLOWS_DIR`); runs write to `~/.android-use/flow-runs/<flow>/<run-id>/`.

```sh
android-use flow list [--app PACKAGE] [--category dice]
android-use flow info dice/open-match
android-use flow validate ./draft.json                  # offline, prints the plan
android-use flow run dice/open-match --input nick=Ann   # events as JSON lines on stderr
android-use flow run dice/open-match --dry-run --from 4 --to 9
android-use flow run dice/open-match --record --logcat --timeout-scale 2
android-use --owner rec flow draft --reset              # start recording this owner's steps
android-use --owner rec flow draft --out ./draft.json   # recorded steps as a v1 draft
android-use flow publish ./draft.json --as dice/open-match --confirm
android-use flow report dice/open-match [--note "Play moved" --confirm]
android-use flow update                                 # git pull, or clone ANDROID_USE_FLOWS_REPO
```

MCP: `phone_flow_list`, `phone_flow_info`, `phone_flow_run`, `phone_flow_draft`, `phone_flow_update`, `phone_flow_report`, `phone_flow_publish`.

Format (v1, closed top-level fields `version, name, description, app, category, risk, locale, tags, inputs, example_inputs, verified_on, steps`):

```json
{
  "version": 1, "name": "Open match", "app": "com.example.dice", "category": "dice", "risk": "navigation",
  "inputs": { "nick": { "description": "player name" } },
  "steps": [
    { "kind": "launch_app", "app": "com.example.dice" },
    { "kind": "wait_for", "expect": { "app": "com.example.dice" }, "timeout_ms": 60000 },
    { "kind": "pause", "ms": 3000, "note": "splash has no elements" },
    { "kind": "mark", "name": "menu_ready" },
    { "kind": "tap", "label": "Play", "element_kind": "Button" },
    { "kind": "tap", "x": 0.5, "y": 0.82, "optional": true },
    { "kind": "type", "input": "nick", "clear": true },
    { "kind": "screenshot", "name": "lobby" }
  ]
}
```

Device steps are the `act` actions (`tap, longpress, swipe, scroll, type, back, home, launch_app, wait_for`) with `kind` in place of `type`; a tap's element kind is `element_kind`. Runner steps never reach the device: `pause` (≤ 60 s; prefer `wait_for`), `mark` (a named timing event) and `screenshot`. `wait_for` may wait up to 300 s. Any step takes `note`; `optional:true` skips a step that failed without sending anything; `settle_ms` observes the screen after it. Only `type` steps reference inputs, and every declared input must be used. `risk:"side_effect"` needs `--confirm`.

Debugging a run:

- `events.jsonl` has `flow_start`, `step_start`, `step_end` (`ok`, `elapsed_ms`, `error`, `outcome`, `retry_safe`), `mark`, `step_skipped` and `flow_end`, each with `ts_ms` (epoch ms), `ts` (UTC) and `t_ms` (since start). On an emulator the device clock is the host clock, so `ts_ms` lines up with `adb logcat -v epoch` and with screen recordings; `--record`/`--logcat` also log a `clock` event with the measured device offset.
- A failure names the step (`step 4/12 (tap label="Play") failed: ambiguous_element — …`), lists visible labels, and saves `failure-elements.json` and `failure.png` in the run directory. `--screenshot-each` keeps one image per step.
- Nothing is retried. When `applied_actions > 0` the run is not replayable: read the screen, then resume with `--from N` as the hint says.
- Before step 1, and again whenever a step fails, the run checks the daemon and the phone (read-only: `adb devices -l`, `dumpsys power`, `dumpsys window`). Each problem has its own code and next action: `device_not_attached`, `device_unauthorized` (accept the "Allow USB debugging?" prompt), `device_offline`, `device_no_permissions`, `screen_off`, `device_locked`, `daemon_not_running`, `daemon_outdated` (prints the exact restart command for this binary), `runner_not_running` and `daemon_stopping`. A preflight refusal sent nothing. `--no-preflight` (MCP `preflight:false`) turns the ADB checks off.
- If the phone or daemon went away during a step (USB disconnect, daemon died), the run ends with a `device_lost` event, `device_error`, `retry_safe:false` and a resume hint: reconnect, `android-use connect`, read the screen, then `--from N` or `--from N+1`. Nothing is replayed automatically.
- `device_busy` names the owner holding the lease and how to free it (`android-use --owner OWNER release`, or `phone_release` in that MCP session).
- `--timeout-scale` stretches both `wait_for` and `pause`. Failure captures replace typed input values with `<input:name>` and editable/password field values with `<redacted>`; screenshots are not redacted. A run's own steps are kept out of the owner's draft recording; `--record-trail` (MCP `record_trail:true`) includes them.
- `android-use doctor` lists every attached device with its state (including `unauthorized`, `offline`, `no permissions`), transport (`usb`, `wifi`, `emulator`), model, whether the screen is awake and locked, and a hint. Real phones are slower than emulators: use `--timeout-scale 2` and `wait_for` rather than short pauses.
- `--record` wraps `adb shell screenrecord` (Android stops it at 180 s) and pulls `screen.mp4`; `--logcat` saves `logcat.txt`. Both need the ADB backend.

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

The ADB installation path grants `WRITE_SECURE_SETTINGS`, preserves other enabled services, exempts the companion from battery optimization and saves the pairing. Once `status` reports `backend:accessibility` and `drivable:true`, the companion runs over Wi-Fi. Stop an existing ADB connection before switching backends. The certificate and token stay private in `~/.android-use`; re-pair after uninstalling the companion or changing the computer address. This initial version uses copy/paste pairing and supports one paired phone per data directory.

A persistent notification and the phone app both offer a stop action. With the one-time settings grant, an enabled companion restores its own accessibility service when it is removed. After an explicit Android force-stop, open the app and start the saved pairing; Android prevents force-stopped apps from restarting themselves. Manual installs need accessibility re-enabled in Settings. On reboot, an enabled companion resumes its saved pairing. Disconnection never replays input.

Release CI builds the APK with a checksum. Tagged releases require `ANDROID_PHONE_KEYSTORE_BASE64`, `ANDROID_PHONE_STORE_PASS` and `ANDROID_PHONE_KEY_PASS` repository secrets; pull requests use a separate development key. Development APKs must not be mistaken for signed release acceptance.


## Questions about Android MCP automation

### Can Claude Code or Codex control Android with android-use?

Yes. Configure `android-use mcp` as a stdio MCP server, or use the CLI from an agent skill. The interface is not tied to one model provider. Nine tools expose status, capabilities, elements, batches, actions, taps, text, screenshots and lease release.

### How does the accessibility companion connect?

The accessibility development preview controls a paired phone over Wi-Fi TLS. The default installation uses ADB once to install, authorize and pair the companion; a manual APK/settings path is also available but has not been hardware-tested without the settings grant. The released ADB backend uses USB debugging.

### Is android-use faster than Appium, UIAutomator or screenshot-based agents?

There is no controlled head-to-head benchmark establishing that. The local native ADB adapter measured 55.07 ms median screen reads on one Samsung Android 16 phone. Batching known steps can reduce agent tool-call round trips; actual model latency and total task time depend on the workflow. The ADB backend itself uses a pinned UIAutomator library with a source-built adapter.

### Does reconnection repeat a tap or text input?

No. Uncertain mutations return `unknown` and are never automatically replayed. Read the screen to determine the postcondition. Five reconnects within one MCP process preserved authorization in the development companion's acceptance run.

### Which Android versions and systems are supported?

The adapters target Android 8/API 26 or newer. Companion screenshots need Android 11/API 30 or newer, and the current companion APK is ARM64. Real-device acceptance covers Samsung SM-A155M with Android 16. Host release binaries cover macOS Intel/Apple Silicon, Linux x64 and Windows x64. Check [releases](https://github.com/leeguooooo/android-use/releases) for published artifacts; a preview source feature is not necessarily in the latest binary release.
