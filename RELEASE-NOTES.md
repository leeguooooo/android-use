android-use 0.2.0 adds saved flows: record a sequence of phone steps once, save it, and replay it with one command (`android-use flow draft|run|list|info|validate|update|report|publish`, plus matching MCP tools).

Flow runs are built for debugging on a real USB phone. Each run writes timestamped step events (events.jsonl) that line up with `--record` screen recordings and `--logcat`. Failures name the step and the reason, save the screen elements and a screenshot, and give a resume hint (`--from N`). `--dry-run` and `flow validate` check a flow without a device.

Before the first step, and after any failed step, the phone is checked: not attached, unauthorized, offline, screen off, locked, daemon not running or outdated, runner not running. Each case has its own error code and next action. A USB disconnect mid-run ends the run without replaying anything. `device_busy` names the owner holding the device and how to release it. Saved failure element trees are redacted.

Known limits: flow recording, `--record` and `--logcat` have been tested with a mocked device but not yet on a physical phone; lock-screen detection may vary by vendor. Publishing flows is local only.
