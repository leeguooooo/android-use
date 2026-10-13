package com.androiduse.phone;
import android.app.*;
import android.content.*;
import android.os.*;
import android.provider.Settings;
import android.util.Base64;
import android.widget.*;
import org.json.*;

public final class MainActivity extends Activity {
    private EditText input;
    private TextView state;
    @Override public void onCreate(Bundle bundle) {
        super.onCreate(bundle);
        LinearLayout layout = new LinearLayout(this); layout.setOrientation(1); layout.setPadding(32,40,32,32);
        state = new TextView(this); state.setText("Paste pairing text from android-use pair. Only pair a computer you trust.\nThe computer can read your screen and send input while enabled."); layout.addView(state);
        input = new EditText(this); input.setHint("Private pairing JSON"); input.setMinLines(3); layout.addView(input);
        Button start = new Button(this); start.setText("Save pairing and start"); layout.addView(start); start.setOnClickListener(v -> save(input.getText().toString()));
        Button resume = new Button(this); resume.setText("Start saved pairing"); layout.addView(resume); resume.setOnClickListener(v -> start());
        Button settings = new Button(this); settings.setText("Accessibility settings"); layout.addView(settings); settings.setOnClickListener(v -> startActivity(new Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)));
        Button stop = new Button(this); stop.setText("Stop and disable control"); layout.addView(stop); stop.setOnClickListener(v -> {
            getSharedPreferences("control",0).edit().putBoolean("running",false).commit();
            startForegroundService(new Intent(this,LinkService.class).setAction("stop")); state.setText("Stopped. Manual installations: turn the service off in Accessibility settings.");
        });
        setContentView(layout);
        if(Build.VERSION.SDK_INT >= 33 && checkSelfPermission("android.permission.POST_NOTIFICATIONS") != android.content.pm.PackageManager.PERMISSION_GRANTED) requestPermissions(new String[]{"android.permission.POST_NOTIFICATIONS"},1);

    }
    private void save(String text) {
        try {
            if(text.length()>24000) throw new Exception();
            JSONObject config = new JSONObject(text);
            if(config.getInt("version")!=1 || !config.getString("endpoint").startsWith("wss://") || config.getString("token").length()!=64 || config.getString("fingerprint").length()!=64) throw new Exception();
            getSharedPreferences("control",0).edit().putString("pairing",text).commit(); input.setText(""); start();
        } catch(Exception e) { state.setText("Invalid pairing. Paste the complete JSON from the terminal."); }
    }
    private void start() {
        if(getSharedPreferences("control",0).getString("pairing","").isEmpty()) { state.setText("Pair first"); return; }
        getSharedPreferences("control",0).edit().putBoolean("running",true).commit();
        startForegroundService(new Intent(this,LinkService.class)); state.setText("Control enabled. Keep this phone and the paired computer on the same Wi-Fi. Enable Accessibility in Settings if not installed with ADB.");
    }
}
