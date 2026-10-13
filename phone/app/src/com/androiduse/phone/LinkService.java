package com.androiduse.phone;
import android.app.*;
import android.content.*;
import android.content.pm.PackageManager;
import android.os.*;
import android.provider.Settings;
import java.util.*;

public final class LinkService extends Service {
    static { System.loadLibrary("android_use_phone"); }
    private native void startNative(String config);
    private native void stopNative();
    public String execute(String call) { return PocketService.execute(call); }
    private String activeConfig = "";
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Runnable heal = new Runnable() { public void run() { restore(); handler.postDelayed(this,5000); } };
    @Override public IBinder onBind(Intent intent) { return null; }
    @Override public void onCreate() {
        super.onCreate();
        NotificationManager manager = getSystemService(NotificationManager.class);
        manager.createNotificationChannel(new NotificationChannel("control",getString(R.string.notif_channel),NotificationManager.IMPORTANCE_LOW));
        Intent stop = new Intent(this,LinkService.class).setAction("stop");
        PendingIntent stopAction = PendingIntent.getService(this,1,stop,PendingIntent.FLAG_IMMUTABLE|PendingIntent.FLAG_UPDATE_CURRENT);
        PendingIntent open = PendingIntent.getActivity(this,2,new Intent(this,MainActivity.class),PendingIntent.FLAG_IMMUTABLE|PendingIntent.FLAG_UPDATE_CURRENT);
        Notification notice = new Notification.Builder(this,"control").setContentTitle(getString(R.string.notif_title))
            .setContentText(getString(R.string.notif_text))
            .setSmallIcon(R.drawable.ic_stat_control).setContentIntent(open).setOngoing(true)
            .addAction(new Notification.Action.Builder(null,getString(R.string.notif_stop),stopAction).build()).build();
        startForeground(1,notice);
    }
    @Override public int onStartCommand(Intent intent,int flags,int id) {
        if (intent != null && "stop".equals(intent.getAction())) { stopControl(); stopSelf(); return START_NOT_STICKY; }
        SharedPreferences prefs = getSharedPreferences("control",0);
        if (!prefs.getBoolean("running",false)) { stopSelf(); return START_NOT_STICKY; }
        String config = prefs.getString("pairing","");
        if (!config.isEmpty() && !config.equals(activeConfig)) { startNative(config); activeConfig=config; }
        handler.removeCallbacks(heal); handler.post(heal);
        return START_STICKY;
    }
    private void restore() {
        if (!getSharedPreferences("control",0).getBoolean("running",false)
            || checkSelfPermission("android.permission.WRITE_SECURE_SETTINGS") != PackageManager.PERMISSION_GRANTED) return;
        String name = getPackageName()+"/"+PocketService.class.getName();
        String enabled = Settings.Secure.getString(getContentResolver(),Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES);
        List<String> services = new ArrayList<>();
        if (enabled != null) for(String s:enabled.split(":")) if(!s.isEmpty()) services.add(s);
        if (!services.contains(name) && !services.contains(getPackageName()+"/.PocketService")) {
            services.add(name);
            try {
                Settings.Secure.putString(getContentResolver(),Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,String.join(":",services));
                Settings.Secure.putInt(getContentResolver(),Settings.Secure.ACCESSIBILITY_ENABLED,1);
            } catch(SecurityException ignored) {}
        }
    }
    private void stopControl() {
        getSharedPreferences("control",0).edit().putBoolean("running",false).commit();
        stopNative(); activeConfig=""; handler.removeCallbacks(heal); PocketService.disable();
        try {
            String enabled = Settings.Secure.getString(getContentResolver(),Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES);
            List<String> services = new ArrayList<>();
            if(enabled!=null) for(String s:enabled.split(":")) if(!s.equals(getPackageName()+"/"+PocketService.class.getName()) && !s.equals(getPackageName()+"/.PocketService") && !s.isEmpty()) services.add(s);
            Settings.Secure.putString(getContentResolver(),Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,String.join(":",services));
        } catch(SecurityException ignored) { /* Manual installations can close it in Settings. */ }
    }
    @Override public void onDestroy() { stopNative(); handler.removeCallbacks(heal); super.onDestroy(); }
}
