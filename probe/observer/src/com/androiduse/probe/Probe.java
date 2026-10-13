package com.androiduse.probe;

import android.accessibilityservice.AccessibilityServiceInfo;
import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Bundle;
import android.provider.Settings;
import android.text.Editable;
import android.text.TextWatcher;
import android.view.Gravity;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.accessibility.AccessibilityManager;
import android.view.accessibility.AccessibilityNodeInfo;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputConnectionWrapper;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import java.io.BufferedReader;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStreamReader;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.List;
import org.json.JSONArray;
import org.json.JSONObject;

/**
 * Observer-only probe: records what an ordinary third-party app can see about
 * debugging, accessibility and input. It never changes device state.
 */
public class Probe extends Activity {
    private static final int[] PORTS = {19008, 9008, 7912, 5037, 5555, 27042};
    private static final String[] PACKAGES = {
        "com.androiduse.stub", "com.github.uiautomator", "com.github.uiautomator.test"
    };
    private static final String[] PROPS = {
        "sys.usb.state", "sys.usb.config", "persist.sys.usb.config", "init.svc.adbd",
        "service.adb.root", "ro.debuggable", "ro.secure"
    };

    private final JSONArray gestures = new JSONArray();
    private final JSONArray inputLog = new JSONArray();
    private final int[] nodeQueries = new int[2]; // target, input field
    private JSONArray current;
    private EditText mode;
    private TextView output;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        int pad = dp(16);
        root.setPadding(pad, pad, pad, pad);

        mode = new EditText(this);
        mode.setHint("mode label");
        mode.setText("unlabeled");
        mode.setSingleLine(true);
        root.addView(mode);

        LinearLayout buttons = new LinearLayout(this);
        Button collect = new Button(this);
        collect.setText("Collect");
        collect.setOnClickListener(v -> collect());
        Button clear = new Button(this);
        clear.setText("Clear");
        clear.setOnClickListener(v -> clearLogs());
        buttons.addView(collect);
        buttons.addView(clear);
        root.addView(buttons);

        TextView target = new TapTarget(this);
        target.setText("TAP TARGET");
        target.setGravity(Gravity.CENTER);
        target.setBackgroundColor(0xFFDDE6F0);
        target.setOnTouchListener((v, event) -> { recordTouch(v, event); return true; });
        root.addView(target, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, dp(160)));

        EditText field = new LoggingEditText(this);
        field.setHint("Input test field");
        field.addTextChangedListener(new TextWatcher() {
            public void beforeTextChanged(CharSequence s, int start, int count, int after) {}
            public void onTextChanged(CharSequence s, int start, int before, int count) {
                log(new J().put("kind", "watcher").put("start", start).put("before", before).put("count", count).put("length", s.length()));
            }
            public void afterTextChanged(Editable s) {}
        });
        root.addView(field);

        output = new TextView(this);
        output.setTextIsSelectable(true);
        root.addView(output);

        ScrollView scroll = new ScrollView(this);
        scroll.addView(root);
        setContentView(scroll);

        // Unattended runs (e.g. while debugging is off) pass the label as an extra.
        String auto = getIntent().getStringExtra("auto_collect");
        if (auto != null) {
            mode.setText(auto);
            output.postDelayed(this::collect, 1500);
        }
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }

    private void clearLogs() {
        for (int i = gestures.length() - 1; i >= 0; i--) gestures.remove(i);
        for (int i = inputLog.length() - 1; i >= 0; i--) inputLog.remove(i);
        nodeQueries[0] = nodeQueries[1] = 0;
        output.setText("cleared");
    }

    private void log(J entry) {
        if (inputLog.length() < 500) inputLog.put(entry.put("t", android.os.SystemClock.uptimeMillis()));
    }

    private void recordTouch(View view, MotionEvent event) {
        int action = event.getActionMasked();
        if (action == MotionEvent.ACTION_DOWN) {
            current = new JSONArray();
            if (gestures.length() < 50) gestures.put(new J()
                .put("dx_from_center", event.getX() - view.getWidth() / 2f)
                .put("dy_from_center", event.getY() - view.getHeight() / 2f)
                .put("events", current));
        }
        if (current == null || current.length() >= 200) return;
        InputDevice device = event.getDevice();
        J row = new J()
            .put("action", action)
            .put("dt", event.getEventTime() - event.getDownTime())
            .put("device_id", event.getDeviceId())
            .put("device_null", device == null)
            .put("device_name", device == null ? "" : device.getName())
            .put("device_virtual", device != null && device.isVirtual())
            .put("source", event.getSource())
            .put("tool_type", event.getToolType(0))
            .put("pressure", event.getPressure())
            .put("size", event.getSize())
            .put("touch_major", event.getTouchMajor())
            .put("touch_minor", event.getTouchMinor())
            .put("orientation", event.getOrientation())
            .put("flags", event.getFlags())
            .put("meta", event.getMetaState())
            .put("buttons", event.getButtonState())
            .put("history", event.getHistorySize())
            .put("x", event.getX())
            .put("y", event.getY());
        if (Build.VERSION.SDK_INT >= 29) row.put("classification", event.getClassification());
        current.put(row);
    }

    private void collect() {
        final String label = mode.getText().toString().trim();
        output.setText("collecting…");
        new Thread(() -> {
            J result;
            try {
                result = snapshot(label);
                String name = "probe-" + label.replaceAll("[^A-Za-z0-9_-]", "_") + "-" + System.currentTimeMillis() + ".json";
                File file = new File(getExternalFilesDir(null), name);
                try (FileOutputStream out = new FileOutputStream(file)) {
                    out.write(result.toString(2).getBytes(StandardCharsets.UTF_8));
                }
                result.put("file", file.getAbsolutePath());
            } catch (Exception e) {
                result = new J().put("error", String.valueOf(e));
            }
            final String text = summary(result);
            runOnUiThread(() -> output.setText(text));
        }).start();
    }

    private J snapshot(String label) throws Exception {
        J r = new J();
        r.put("mode", label)
            .put("time_ms", System.currentTimeMillis())
            .put("model", Build.MANUFACTURER + " " + Build.MODEL)
            .put("sdk_int", Build.VERSION.SDK_INT)
            .put("release", Build.VERSION.RELEASE)
            .put("target_sdk", getApplicationInfo().targetSdkVersion);

        JSONObject settings = new J();
        settings.put("adb_enabled", global(Settings.Global.ADB_ENABLED));
        settings.put("development_settings_enabled", global(Settings.Global.DEVELOPMENT_SETTINGS_ENABLED));
        settings.put("adb_wifi_enabled", global("adb_wifi_enabled"));
        settings.put("accessibility_enabled", secure(Settings.Secure.ACCESSIBILITY_ENABLED));
        settings.put("enabled_accessibility_services", secure(Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES));
        r.put("settings", settings);

        JSONObject props = new J();
        for (String name : PROPS) props.put(name, getprop(name));
        r.put("props", props);

        r.put("usb_state", usbState());

        AccessibilityManager am = (AccessibilityManager) getSystemService(Context.ACCESSIBILITY_SERVICE);
        JSONObject a11y = new J()
            .put("is_enabled", am.isEnabled())
            .put("touch_exploration", am.isTouchExplorationEnabled());
        JSONArray enabled = new JSONArray();
        for (AccessibilityServiceInfo info : am.getEnabledAccessibilityServiceList(AccessibilityServiceInfo.FEEDBACK_ALL_MASK)) enabled.put(info.getId());
        a11y.put("enabled_services", enabled);
        JSONArray installed = new JSONArray();
        for (AccessibilityServiceInfo info : am.getInstalledAccessibilityServiceList()) installed.put(info.getId());
        a11y.put("installed_services", installed);
        a11y.put("node_queries_target", nodeQueries[0]).put("node_queries_input", nodeQueries[1]);
        r.put("accessibility", a11y);

        PackageManager pm = getPackageManager();
        JSONObject packages = new J();
        for (String pkg : PACKAGES) {
            JSONObject p = new J();
            try {
                pm.getPackageInfo(pkg, 0);
                p.put("visible", true);
                p.put("write_secure_settings", pm.checkPermission("android.permission.WRITE_SECURE_SETTINGS", pkg) == PackageManager.PERMISSION_GRANTED);
            } catch (PackageManager.NameNotFoundException e) {
                p.put("visible", false);
            }
            packages.put(pkg, p);
        }
        r.put("packages", packages);

        JSONObject ports = new J();
        for (int port : PORTS) ports.put(String.valueOf(port), probePort(port));
        r.put("loopback_open", ports);
        r.put("proc_net_tcp_readable", new File("/proc/net/tcp").canRead());
        r.put("tmp_runner_jar_visible", new File("/data/local/tmp/android-use-u2-0.4.0-adapter.jar").exists());

        r.put("gestures", gestures);
        r.put("input_log", inputLog);
        return r;
    }

    private String global(String key) {
        try { return Settings.Global.getString(getContentResolver(), key); } catch (Exception e) { return "error:" + e.getClass().getSimpleName(); }
    }

    private String secure(String key) {
        try { return Settings.Secure.getString(getContentResolver(), key); } catch (Exception e) { return "error:" + e.getClass().getSimpleName(); }
    }

    private static String getprop(String name) {
        try {
            Process p = Runtime.getRuntime().exec(new String[]{"getprop", name});
            try (BufferedReader in = new BufferedReader(new InputStreamReader(p.getInputStream(), StandardCharsets.UTF_8))) {
                String line = in.readLine();
                p.waitFor();
                return line == null ? "" : line;
            }
        } catch (Exception e) {
            return "error:" + e.getClass().getSimpleName();
        }
    }

    private J usbState() {
        J usb = new J();
        IntentFilter filter = new IntentFilter("android.hardware.usb.action.USB_STATE");
        Intent sticky = Build.VERSION.SDK_INT >= 33
            ? registerReceiver(null, filter, Context.RECEIVER_EXPORTED)
            : registerReceiver(null, filter);
        if (sticky == null || sticky.getExtras() == null) return usb.put("available", false);
        usb.put("available", true);
        for (String key : sticky.getExtras().keySet()) usb.put(key, String.valueOf(sticky.getExtras().get(key)));
        return usb;
    }

    private static boolean probePort(int port) {
        try (Socket socket = new Socket()) {
            socket.connect(new InetSocketAddress("127.0.0.1", port), 150);
            return true;
        } catch (Exception e) {
            return false;
        }
    }

    private static String summary(JSONObject r) {
        if (r.has("error")) return "error: " + r.optString("error");
        JSONObject s = r.optJSONObject("settings"), a = r.optJSONObject("accessibility"), u = r.optJSONObject("usb_state");
        StringBuilder b = new StringBuilder();
        b.append("mode=").append(r.optString("mode")).append('\n');
        b.append("adb_enabled=").append(s.optString("adb_enabled")).append(" dev=").append(s.optString("development_settings_enabled")).append('\n');
        b.append("usb adb=").append(u.optString("adb", "-")).append(" connected=").append(u.optString("connected", "-")).append('\n');
        b.append("init.svc.adbd=").append(r.optJSONObject("props").optString("init.svc.adbd")).append('\n');
        b.append("a11y enabled=").append(a.optBoolean("is_enabled")).append(" services=").append(a.optJSONArray("enabled_services")).append('\n');
        b.append("node queries target/input=").append(a.optInt("node_queries_target")).append('/').append(a.optInt("node_queries_input")).append('\n');
        b.append("loopback=").append(r.optJSONObject("loopback_open")).append('\n');
        b.append("gestures=").append(r.optJSONArray("gestures").length()).append(" input events=").append(r.optJSONArray("input_log").length()).append('\n');
        b.append("file=").append(r.optString("file"));
        return b.toString();
    }

    /** Counts accessibility reads and actions an app can observe on its own views. */
    private final class TapTarget extends TextView {
        TapTarget(Context context) { super(context); }
        @Override public void onInitializeAccessibilityNodeInfo(AccessibilityNodeInfo info) {
            super.onInitializeAccessibilityNodeInfo(info);
            nodeQueries[0]++;
        }
        @Override public boolean performAccessibilityAction(int action, Bundle arguments) {
            log(new J().put("kind", "a11y_action").put("view", "target").put("action", action));
            return super.performAccessibilityAction(action, arguments);
        }
    }

    private final class LoggingEditText extends EditText {
        LoggingEditText(Context context) { super(context); }
        @Override public void onInitializeAccessibilityNodeInfo(AccessibilityNodeInfo info) {
            super.onInitializeAccessibilityNodeInfo(info);
            nodeQueries[1]++;
        }
        @Override public boolean performAccessibilityAction(int action, Bundle arguments) {
            log(new J().put("kind", "a11y_action").put("view", "input").put("action", action));
            return super.performAccessibilityAction(action, arguments);
        }
        @Override public boolean dispatchKeyEvent(KeyEvent event) {
            log(new J().put("kind", "key").put("action", event.getAction()).put("code", event.getKeyCode())
                .put("device_id", event.getDeviceId()).put("source", event.getSource()).put("flags", event.getFlags()));
            return super.dispatchKeyEvent(event);
        }
        @Override public InputConnection onCreateInputConnection(EditorInfo info) {
            InputConnection base = super.onCreateInputConnection(info);
            if (base == null) return null;
            return new InputConnectionWrapper(base, true) {
                @Override public boolean commitText(CharSequence text, int position) {
                    log(new J().put("kind", "ic_commit").put("length", text.length()));
                    return super.commitText(text, position);
                }
                @Override public boolean setComposingText(CharSequence text, int position) {
                    log(new J().put("kind", "ic_composing").put("length", text.length()));
                    return super.setComposingText(text, position);
                }
                @Override public boolean deleteSurroundingText(int before, int after) {
                    log(new J().put("kind", "ic_delete").put("before", before).put("after", after));
                    return super.deleteSurroundingText(before, after);
                }
                @Override public boolean sendKeyEvent(KeyEvent event) {
                    log(new J().put("kind", "ic_key").put("action", event.getAction()).put("code", event.getKeyCode()));
                    return super.sendKeyEvent(event);
                }
            };
        }
    }
}
