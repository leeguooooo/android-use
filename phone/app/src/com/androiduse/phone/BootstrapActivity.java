package com.androiduse.phone;
import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;
import android.util.Base64;
import org.json.JSONObject;

/** One-time adb bootstrap; manifest DUMP permission excludes ordinary apps. */
public final class BootstrapActivity extends Activity {
    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        try {
            String text = new String(Base64.decode(getIntent().getStringExtra("pairing"),Base64.DEFAULT),"UTF-8");
            if(text.length()>24000) throw new Exception();
            JSONObject pair = new JSONObject(text);
            if(pair.getInt("version")!=1 || !pair.getString("endpoint").startsWith("wss://") || pair.getString("token").length()!=64) throw new Exception();
            getSharedPreferences("control",0).edit().putString("pairing",text).putBoolean("running",true).commit();
            startForegroundService(new Intent(this,LinkService.class));
        } catch(Exception ignored) {}
        getIntent().removeExtra("pairing"); finish();
    }
}
