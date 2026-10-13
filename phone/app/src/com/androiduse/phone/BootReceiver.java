package com.androiduse.phone;
import android.content.*;
public final class BootReceiver extends BroadcastReceiver {
    @Override public void onReceive(Context context,Intent intent) {
        if (Intent.ACTION_BOOT_COMPLETED.equals(intent.getAction()) && context.getSharedPreferences("control",0).getBoolean("running",false)) context.startForegroundService(new Intent(context,LinkService.class));
    }
}
