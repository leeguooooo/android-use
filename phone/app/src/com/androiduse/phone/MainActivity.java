package com.androiduse.phone;
import android.app.*;
import android.content.*;
import android.graphics.*;
import android.os.*;
import android.provider.Settings;
import android.view.*;
import android.widget.*;
import org.json.*;

public final class MainActivity extends Activity {
    private EditText input;
    private TextView state;
    @Override public void onCreate(Bundle bundle) {
        super.onCreate(bundle);
        setContentView(R.layout.main);
        input = findViewById(R.id.input);
        state = findViewById(R.id.status);
        findViewById(R.id.btn_save).setOnClickListener(v -> save(input.getText().toString()));
        findViewById(R.id.btn_resume).setOnClickListener(v -> start());
        findViewById(R.id.btn_accessibility).setOnClickListener(v -> startActivity(new Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)));
        findViewById(R.id.btn_stop).setOnClickListener(v -> stop());
        applyInsets();
        if(Build.VERSION.SDK_INT >= 33 && checkSelfPermission("android.permission.POST_NOTIFICATIONS") != android.content.pm.PackageManager.PERMISSION_GRANTED) requestPermissions(new String[]{"android.permission.POST_NOTIFICATIONS"},1);
    }
    @Override protected void onResume() { super.onResume(); showState(); }
    private void applyInsets() {
        final View header = findViewById(R.id.header);
        final View content = findViewById(R.id.content);
        final int headerTop = header.getPaddingTop();
        final int contentBottom = content.getPaddingBottom();
        findViewById(R.id.root).setOnApplyWindowInsetsListener((v, insets) -> {
            int top, bottom;
            if (Build.VERSION.SDK_INT >= 30) {
                Insets bars = insets.getInsets(WindowInsets.Type.systemBars());
                top = bars.top; bottom = bars.bottom;
            } else {
                top = insets.getSystemWindowInsetTop();
                bottom = insets.getSystemWindowInsetBottom();
            }
            header.setPadding(header.getPaddingLeft(), headerTop + top, header.getPaddingRight(), header.getPaddingBottom());
            content.setPadding(content.getPaddingLeft(), content.getPaddingTop(), content.getPaddingRight(), contentBottom + bottom);
            return insets;
        });
    }
    private void showState() {
        SharedPreferences prefs = getSharedPreferences("control",0);
        if(prefs.getBoolean("running",false)) state.setText(R.string.status_running);
        else if(!prefs.getString("pairing","").isEmpty()) state.setText(R.string.status_paired_idle);
        else state.setText(R.string.status_idle);
    }
    private void save(String text) {
        try {
            if(text.length()>24000) throw new Exception();
            JSONObject config = new JSONObject(text);
            if(config.getInt("version")!=1 || !config.getString("endpoint").startsWith("wss://") || config.getString("token").length()!=64 || config.getString("fingerprint").length()!=64) throw new Exception();
            getSharedPreferences("control",0).edit().putString("pairing",text).commit(); input.setText(""); start();
        } catch(Exception e) { state.setText(R.string.status_invalid); }
    }
    private void start() {
        if(getSharedPreferences("control",0).getString("pairing","").isEmpty()) { state.setText(R.string.status_pair_first); return; }
        getSharedPreferences("control",0).edit().putBoolean("running",true).commit();
        startForegroundService(new Intent(this,LinkService.class)); state.setText(R.string.status_running);
    }
    private void stop() {
        getSharedPreferences("control",0).edit().putBoolean("running",false).commit();
        startForegroundService(new Intent(this,LinkService.class).setAction("stop")); state.setText(R.string.status_stopped);
    }
}
