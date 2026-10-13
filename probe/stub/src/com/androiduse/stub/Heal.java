package com.androiduse.stub;

import android.app.Activity;
import android.content.pm.PackageManager;
import android.os.Bundle;
import android.provider.Settings;
import android.widget.TextView;

/** On open: re-add StubService to the enabled list if WRITE_SECURE_SETTINGS allows. */
public class Heal extends Activity {
    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        TextView view = new TextView(this);
        view.setPadding(48, 48, 48, 48);
        view.setTextIsSelectable(true);
        view.setText(heal());
        setContentView(view);
    }

    private String heal() {
        String self = getPackageName() + "/" + StubService.class.getName();
        String shortSelf = getPackageName() + "/." + StubService.class.getSimpleName();
        String enabled = Settings.Secure.getString(getContentResolver(), Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES);
        if (enabled == null) enabled = "";
        boolean granted = checkSelfPermission("android.permission.WRITE_SECURE_SETTINGS") == PackageManager.PERMISSION_GRANTED;
        boolean present = false;
        for (String entry : enabled.split(":")) present |= entry.equals(self) || entry.equals(shortSelf);
        String result;
        if (present) {
            result = "already_enabled";
        } else if (!granted) {
            result = "not_enabled_no_permission";
        } else {
            try {
                Settings.Secure.putString(getContentResolver(), Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES, enabled.isEmpty() ? self : enabled + ":" + self);
                Settings.Secure.putInt(getContentResolver(), Settings.Secure.ACCESSIBILITY_ENABLED, 1);
                result = "restored";
            } catch (SecurityException e) {
                result = "write_denied";
            }
        }
        return "result=" + result + "\nwrite_secure_settings=" + granted + "\nenabled_before=" + enabled;
    }
}
