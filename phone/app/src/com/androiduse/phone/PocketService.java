package com.androiduse.phone;
import android.accessibilityservice.*;
import android.graphics.*;
import android.os.*;
import android.view.*;
import android.view.accessibility.*;
import android.content.*;
import android.util.Base64;
import org.json.*;
import java.util.*;
import java.util.concurrent.*;
import java.io.ByteArrayOutputStream;

/** Android framework bridge. TLS, protocol validation and reconnection are Rust. */
public final class PocketService extends AccessibilityService {
    private static volatile PocketService instance;
    private static int rows;
    private static final class Refused extends Exception {
        final String code;
        Refused(String code) { this.code = code; }
    }
    @Override protected void onServiceConnected() {
        instance = this;
        if (getSharedPreferences("control",0).getBoolean("running",false)) startForegroundService(new Intent(this,LinkService.class));
    }
    @Override public void onDestroy() { if (instance == this) instance = null; super.onDestroy(); }
    static void disable() { PocketService service=instance; if(service!=null) service.disableSelf(); }
    @Override public boolean onUnbind(Intent intent) { if(instance==this) instance=null; return super.onUnbind(intent); }
    @Override public void onAccessibilityEvent(AccessibilityEvent event) {}
    @Override public void onInterrupt() {}
    private static AccessibilityNodeInfo root() { return instance.getRootInActiveWindow(); }
    static synchronized String execute(String text) {
        JSONObject result = new JSONObject();
        try {
            if (instance == null) throw new Refused("accessibility_unavailable");
            JSONObject call = new JSONObject(text); JSONArray p = call.getJSONArray("params");
            Object value;
            switch (call.getString("method")) {
                case "screen": value = screen(); break;
                case "typeText": value = typeText(p.getString(0),p.getBoolean(1)); break;
                case "click": value = gesture(p.getInt(0),p.getInt(1),p.getInt(0),p.getInt(1),p.length()==3?p.getLong(2):50); break;
                case "swipe": value = gesture(p.getInt(0),p.getInt(1),p.getInt(2),p.getInt(3),p.getLong(4)*5); break;
                case "pressKey": value = instance.performGlobalAction("back".equals(p.getString(0))?GLOBAL_ACTION_BACK:GLOBAL_ACTION_HOME); break;
                case "launchApp":
                    Intent intent = instance.getPackageManager().getLaunchIntentForPackage(p.getString(0));
                    if (intent == null) throw new Refused("app_not_found");
                    intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK); instance.startActivity(intent); value = true; break;
                case "screenshot": value = screenshot(); break;
                default: throw new Refused("unsupported_method");
            }
            result.put("result",value);
        } catch (Refused e) {
            try { result.put("error",new JSONObject().put("code",-32001).put("message",e.code).put("data",new JSONObject().put("dispatched",false))); } catch(Exception ignored) {}
        } catch (Throwable e) {
            try { result.put("error",new JSONObject().put("code",-32002).put("message","phone_operation_failed")); } catch(Exception ignored) {}
        }
        return result.toString();
    }
    private static JSONObject screen() throws Exception {
        AccessibilityNodeInfo active = root(); if (active == null) throw new Refused("screen_unavailable");
        Point size = new Point(); ((WindowManager)instance.getSystemService(WINDOW_SERVICE)).getDefaultDisplay().getRealSize(size);
        String app = string(active.getPackageName()); JSONArray elements = new JSONArray(); rows = 0;
        List<AccessibilityWindowInfo> windows = instance.getWindows();
        if (windows == null || windows.isEmpty()) walk(active,0,elements);
        else for (AccessibilityWindowInfo window: windows) {
            AccessibilityNodeInfo node = window.getRoot();
            if (node != null) { try { walk(node,0,elements); } finally { node.recycle(); } }
        }
        active.recycle();
        return new JSONObject().put("api",Build.VERSION.SDK_INT).put("device","").put("app",app).put("width",size.x).put("height",size.y).put("elements",elements);
    }
    private static boolean gesture(int x1,int y1,int x2,int y2,long ms) throws Exception {
        if (ms < 1 || ms > 10000) throw new Refused("invalid_duration");
        PocketService service = instance;
        Path path = new Path(); path.moveTo(x1,y1); if(x1!=x2 || y1!=y2) path.lineTo(x2,y2);
        GestureDescription gesture = new GestureDescription.Builder().addStroke(new GestureDescription.StrokeDescription(path,0,ms)).build();
        CountDownLatch done = new CountDownLatch(1); boolean[] success = {false};
        new Handler(Looper.getMainLooper()).post(() -> {
            boolean accepted = service.dispatchGesture(gesture,new GestureResultCallback() {
                @Override public void onCompleted(GestureDescription g) { success[0]=true; done.countDown(); }
                @Override public void onCancelled(GestureDescription g) { done.countDown(); }
            },null);
            if(!accepted) done.countDown();
        });
        return done.await(ms+1500,TimeUnit.MILLISECONDS) && success[0];
    }
    private static String screenshot() throws Exception {
        if (Build.VERSION.SDK_INT < 30) throw new Refused("screenshot_unavailable");
        CountDownLatch done = new CountDownLatch(1); String[] output = {null};
        instance.takeScreenshot(Display.DEFAULT_DISPLAY,instance.getMainExecutor(),new TakeScreenshotCallback() {
            @Override public void onSuccess(ScreenshotResult result) {
                try {
                    Bitmap hardware = Bitmap.wrapHardwareBuffer(result.getHardwareBuffer(),result.getColorSpace());
                    if (hardware != null) {
                        Bitmap bitmap = hardware.copy(Bitmap.Config.ARGB_8888,false);
                        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
                        bitmap.compress(Bitmap.CompressFormat.PNG,100,bytes);
                        if (bytes.size() <= 16*1024*1024) output[0] = Base64.encodeToString(bytes.toByteArray(),Base64.NO_WRAP);
                        bitmap.recycle(); hardware.recycle();
                    }
                } finally { result.getHardwareBuffer().close(); done.countDown(); }
            }
            @Override public void onFailure(int error) { done.countDown(); }
        });
        if (!done.await(4,TimeUnit.SECONDS) || output[0] == null) throw new Refused("screenshot_unavailable");
        return output[0];
    }
    private static String string(CharSequence value) { return value == null ? "" : value.toString(); }
    private static void walk(AccessibilityNodeInfo node,int depth,JSONArray out) throws Exception {
        if (++rows > 10000 || depth > 50) throw new Refused("hierarchy_too_large"); // profile-use: allow; technical traversal bound
        String cls = string(node.getClassName()); String text = string(node.getText()); String desc = string(node.getContentDescription());
        boolean editable = node.isEditable(); boolean hint = Build.VERSION.SDK_INT >= 26 && node.isShowingHintText();
        String kind = editable ? "TextField" : cls.endsWith("ProgressBar") ? "ProgressIndicator" : cls.endsWith("Switch") || node.isCheckable() ? "Switch" : node.isClickable() || cls.endsWith("Button") ? "Button" : node.isScrollable() ? "ScrollView" : !text.isEmpty() || !desc.isEmpty() ? "StaticText" : "Other";
        if (!"Other".equals(kind)) {
            Rect r = new Rect(); node.getBoundsInScreen(r);
            JSONObject row = new JSONObject().put("kind",kind).put("label",node.isPassword() ? "<password>" : !text.isEmpty() ? text : desc)
                .put("rect",new JSONArray().put(r.left).put(r.top).put(Math.max(0,r.width())).put(Math.max(0,r.height())))
                .put("depth",depth).put("enabled",node.isEnabled()).put("visible",node.isVisibleToUser()).put("focused",node.isFocused()).put("password",node.isPassword());
            String id = node.getViewIdResourceName(); if (id != null && !id.isEmpty()) row.put("identifier",id);
            if (editable) row.put("value",node.isPassword() || hint ? "" : text);
            if (node.isCheckable()) row.put("value",node.isChecked() ? "1" : "0");
            out.put(row);
        }
        for (int i=0;i<node.getChildCount();i++) {
            AccessibilityNodeInfo child = node.getChild(i);
            if (child != null) { walk(child,depth+1,out); child.recycle(); }
        }
    }
    private static AccessibilityNodeInfo focusedInput() {
        AccessibilityNodeInfo active = root();
        if (active != null) {
            AccessibilityNodeInfo focus = active.findFocus(AccessibilityNodeInfo.FOCUS_INPUT); active.recycle();
            if (focus != null && focus.isEditable() && focus.isVisibleToUser()) return focus;
            if (focus != null) focus.recycle();
        }
        for (AccessibilityWindowInfo window : instance.getWindows()) {
            if (window.getType() != AccessibilityWindowInfo.TYPE_APPLICATION) continue;
            AccessibilityNodeInfo node = window.getRoot(); if (node == null) continue;
            AccessibilityNodeInfo focus = node.findFocus(AccessibilityNodeInfo.FOCUS_INPUT); node.recycle();
            if (focus != null && focus.isEditable() && focus.isVisibleToUser()) return focus;
            if (focus != null) focus.recycle();
        }
        return null;
    }
    private static JSONObject typeText(String text,boolean clear) throws Exception {
        if (text.codePointCount(0,text.length()) > 20000) throw new Refused("text_too_long");
        AccessibilityNodeInfo focus = focusedInput(); if (focus == null) throw new Refused("input_not_focused");
        try {
            boolean password = focus.isPassword();
            if (password && !clear) throw new Refused("password_append_unsupported");
            String old = clear || focus.isShowingHintText() ? "" : string(focus.getText());
            int start = clear ? 0 : focus.getTextSelectionStart(), end = clear ? 0 : focus.getTextSelectionEnd();
            if (start < 0 || end < start || end > old.length()) start = end = old.length();
            String expected = old.substring(0,start) + text + old.substring(end);
            if (expected.codePointCount(0,expected.length()) > 20000) throw new Refused("text_too_long");
            Bundle values = new Bundle(); values.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,expected);
            boolean applied = focus.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT,values);
            boolean verified = false;
            // WebView may acknowledge before its accessibility cache updates.
            // Observe only: never replay ACTION_SET_TEXT to obtain verification.
            if (applied && !password) {
                long deadline = android.os.SystemClock.uptimeMillis() + 400;
                do {
                    if (focus.refresh()) {
                        verified = expected.equals(focus.isShowingHintText() ? "" : string(focus.getText()));
                        if (verified) break;
                    }
                    android.os.SystemClock.sleep(25);
                } while (android.os.SystemClock.uptimeMillis() < deadline);
            }
            return new JSONObject().put("applied",applied).put("verified",verified).put("redacted",password);
        } finally { focus.recycle(); }
    }
}
