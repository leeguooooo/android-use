package com.androiduse.stub;

import android.app.Activity;
import android.content.ComponentName;
import android.content.Intent;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.provider.Settings;
import android.util.Log;

/**
 * Test only. Turns developer options and USB debugging off, lets the probe
 * collect unattended, then restores both. Requires WRITE_SECURE_SETTINGS.
 */
public class DevToggle extends Activity {
    private static final String TAG = "stub-devtoggle";

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        final String label = getIntent().getStringExtra("label") == null ? "dev-off" : getIntent().getStringExtra("label");
        final long holdMs = getIntent().getLongExtra("hold_ms", 12000);
        final Handler handler = new Handler(Looper.getMainLooper());
        set(0);
        handler.postDelayed(() -> {
            Intent probe = new Intent().setComponent(new ComponentName("com.androiduse.probe", "com.androiduse.probe.Probe"))
                .putExtra("auto_collect", label)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK);
            startActivity(probe);
        }, 3000);
        handler.postDelayed(() -> set(1), holdMs);
    }

    private void set(int value) {
        try {
            Settings.Global.putInt(getContentResolver(), Settings.Global.ADB_ENABLED, value);
            Settings.Global.putInt(getContentResolver(), Settings.Global.DEVELOPMENT_SETTINGS_ENABLED, value);
            Log.i(TAG, "developer options set to " + value);
        } catch (SecurityException e) {
            Log.w(TAG, "write denied: " + e);
        }
    }
}
