---
name: android-use
description: Control a real Android phone through authorized ADB with the android-use CLI or MCP server. Use for reading native and accessible WebView screens, tapping, typing Unicode, scrolling, and running batches on Android.
---

# android-use

Install a missing CLI with:

```sh
curl -fsSL https://raw.githubusercontent.com/leeguooooo/android-use/main/install.sh | sh
```

Windows uses the repository's `install.ps1`. Users need Android platform-tools (`adb`) and must authorize USB debugging on their unlocked phone. `android-use doctor` lists attached devices without touching the screen. `android-use connect` starts the background service for the only authorized device; specify `--serial DEVICE_SERIAL` when several are attached. `android-use serve --serial DEVICE_SERIAL` runs it in the foreground. The service uses loopback HTTP and private credentials in `~/.android-use/connection.json`.

Use `android-use mcp` as a stdio MCP server. Each MCP process has its own owner. CLI callers should choose a stable session using `--owner SESSION`. Probe `phone_status` / `android-use status` first; act only when `drivable:true`. Respect another caller's lease.

Read `phone_elements` once and prefer one `phone_run_steps` batch for known steps. Rows have indexes bound to that read's snapshot; use both for taps. Labels must match exactly one visible, enabled row. Coordinates are normalized from 0 to 1. Each action defaults to observing changes, so a separate read after every tap is unnecessary.

```sh
android-use --owner example act '{"steps":[{"type":"launch_app","app":"com.android.settings"},{"type":"wait_for","expect":{"app":"com.android.settings"},"timeout_ms":5000}]}'
android-use --owner example elements
android-use --owner example act '{"type":"tap","element":3,"snapshot":"SNAPSHOT_FROM_READ"}'
```

`wait_for` uses exact `app`, `label`, `identifier` and/or `value` predicates. Multiple element predicates must match the same visible row. Put waits after screen changes. A batch stops on its first failure and reports the completed steps and applied actions separately.

Type only after confirming the foreground app and focused editable field. `phone_type` / `{"type":"type","text":"hello"}` inserts at the current selection; `clear:true` replaces the whole field. Native input preserves Unicode and line breaks without the host clipboard. Password fields support replacement only and their values are redacted. WebView input needs editable accessibility support; use `phone_capabilities` for the running version's limits.

`outcome:unknown` or `retry_safe:false` forbids automatic replay. Read the screen and check the requested postcondition. An acknowledged gesture does not prove navigation or a business operation succeeded. Do not repeat a partially completed batch. If a person changes the screen, re-read before continuing.

Use `phone_screenshot` only when the tree does not answer the question. Protected surfaces may be blank. Release the caller's lease with `phone_release` / `android-use release` when done; the connection stays ready for the next task.
