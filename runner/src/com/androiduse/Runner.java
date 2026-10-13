package com.androiduse;

import android.accessibilityservice.AccessibilityServiceInfo;
import android.app.UiAutomation;
import android.graphics.Rect;
import android.os.Build;
import android.os.Bundle;
import android.view.accessibility.AccessibilityNodeInfo;
import android.view.accessibility.AccessibilityWindowInfo;
import org.json.JSONArray;
import org.json.JSONObject;
import java.io.*;
import java.lang.reflect.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.*;

/** Android-use's source-built adapter. No clipboard, XML text loss or public control port. */
public final class Runner {
    private static UiAutomation automation;
    private static Object service;
    private static String token;
    private static int rows;
    private static final int MAX_BODY = 1024 * 1024;

    private static final class Refused extends Exception {
        final String code;
        Refused(String code) { this.code = code; }
    }

    public static void main(String[] args) throws Exception {
        int port = Integer.parseInt(args[0]);
        token = new String(readFile(args[1]), StandardCharsets.UTF_8).trim();
        if (token.length() < 32) throw new IllegalArgumentException("Invalid control token");
        Class<?> workarounds = Class.forName("com.genymobile.scrcpy.Workarounds");
        workarounds.getMethod("apply", boolean.class, boolean.class).invoke(null, true, true);
        Class<?> wrapperClass = Class.forName("uiautomator.InstrumentShellWrapper");
        Object wrapper = wrapperClass.getMethod("getInstance").invoke(null);
        wrapperClass.getMethod("getContext").invoke(wrapper);
        automation = (UiAutomation)wrapperClass.getMethod("getUiAutomation").invoke(wrapper);
        AccessibilityServiceInfo info = automation.getServiceInfo();
        info.flags |= AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS;
        automation.setServiceInfo(info);
        service = Class.forName("com.wetest.uia2.stub.AutomatorServiceImpl").getConstructor().newInstance();
        Class<?> configurator = Class.forName("androidx.test.uiautomator.Configurator");
        Object config = configurator.getMethod("getInstance").invoke(null);
        configurator.getMethod("setWaitForIdleTimeout", long.class).invoke(config, 0L);
        configurator.getMethod("setWaitForSelectorTimeout", long.class).invoke(config, 0L);
        ServerSocket server = new ServerSocket();
        server.bind(new InetSocketAddress(InetAddress.getByName("127.0.0.1"), port));
        System.err.println("android-use runner ready on loopback");
        while (true) {
            try (Socket socket = server.accept()) {
                socket.setSoTimeout(15000);
                handle(socket);
            } catch (IOException ignored) { /* A lost client never causes replay. */ }
        }
    }

    private static byte[] readFile(String path) throws IOException {
        try (FileInputStream in = new FileInputStream(path); ByteArrayOutputStream out = new ByteArrayOutputStream()) {
            byte[] buffer = new byte[4096]; int n;
            while ((n = in.read(buffer)) > 0) { if (out.size() + n > MAX_BODY) throw new IOException("Too large"); out.write(buffer, 0, n); }
            return out.toByteArray();
        }
    }
    private static String line(InputStream in) throws IOException {
        ByteArrayOutputStream out = new ByteArrayOutputStream(); int b;
        while ((b = in.read()) != -1 && b != '\n') { if (out.size() >= 8192) throw new IOException("Header too large"); if (b != '\r') out.write(b); }
        return out.toString("UTF-8");
    }
    private static void reply(Socket socket, int status, String body) throws IOException {
        byte[] bytes = body.getBytes(StandardCharsets.UTF_8);
        String head = "HTTP/1.1 " + status + " Response\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: " + bytes.length + "\r\nConnection: close\r\n\r\n";
        socket.getOutputStream().write(head.getBytes(StandardCharsets.US_ASCII));
        socket.getOutputStream().write(bytes); socket.getOutputStream().flush();
    }
    private static void handle(Socket socket) throws IOException {
        InputStream in = socket.getInputStream(); String request = line(in);
        Map<String,String> headers = new HashMap<>(); String header;
        while (!(header = line(in)).isEmpty()) {
            int colon = header.indexOf(':');
            if (colon > 0) headers.put(header.substring(0,colon).trim().toLowerCase(Locale.ROOT), header.substring(colon+1).trim());
            if (headers.size() > 64) throw new IOException("Too many headers");
        }
        if (request.startsWith("GET /ping ")) { reply(socket,200,"\"pong\""); return; }
        String authorization = headers.get("authorization");
        if (authorization == null || !MessageDigest.isEqual(authorization.getBytes(StandardCharsets.UTF_8), ("Bearer " + token).getBytes(StandardCharsets.UTF_8))) {
            reply(socket,401,"{\"error\":\"unauthorized\"}"); return;
        }
        if (!request.startsWith("POST /jsonrpc/0 ")) { reply(socket,404,"{}"); return; }
        try {
            int length = Integer.parseInt(headers.getOrDefault("content-length", "0"));
            if (length < 1 || length > MAX_BODY) { reply(socket,413,"{}"); return; }
            byte[] body = new byte[length]; int offset = 0, n;
            while (offset < length && (n = in.read(body,offset,length-offset)) > 0) offset += n;
            if (offset != length) throw new IOException("Incomplete request");
            JSONObject call = new JSONObject(new String(body,StandardCharsets.UTF_8));
            JSONObject result = new JSONObject().put("jsonrpc","2.0").put("id",call.opt("id"));
            try { result.put("result", dispatch(call.getString("method"),call.optJSONArray("params"))); }
            catch (Refused e) { result.put("error",new JSONObject().put("code",-32001).put("message",e.code).put("data",new JSONObject().put("dispatched",false))); }
            catch (Throwable e) { result.put("error",new JSONObject().put("code",-32002).put("message","runner_operation_failed")); }
            reply(socket,200,result.toString());
        } catch (IOException e) { throw e; }
        catch (Exception e) { reply(socket,400,"{\"error\":\"invalid_request\"}"); }
    }
    private static Object dispatch(String method, JSONArray params) throws Exception {
        if ("screen".equals(method)) return screen();
        if ("typeText".equals(method)) return typeText(params.getString(0),params.getBoolean(1));
        Set<String> allowed = new HashSet<>(Arrays.asList("click","swipe","pressKey","pressKeyCode"));
        if (!allowed.contains(method)) throw new Refused("unsupported_method");
        return invoke(method,params);
    }
    private static Object invoke(String name, JSONArray params) throws Exception {
        int count = params == null ? 0 : params.length();
        for (Method method : service.getClass().getMethods()) {
            if (!method.getName().equals(name) || method.getParameterTypes().length != count) continue;
            Class<?>[] types = method.getParameterTypes(); Object[] values = new Object[count]; boolean supported = true;
            for (int i=0;i<count;i++) {
                Object value = params.get(i);
                if (types[i] == int.class && value instanceof Number) values[i] = ((Number)value).intValue();
                else if (types[i] == long.class && value instanceof Number) values[i] = ((Number)value).longValue();
                else if (types[i] == String.class && value instanceof String) values[i] = value;
                else { supported = false; break; }
            }
            if (supported) return method.invoke(service,values);
        }
        throw new Refused("invalid_parameters");
    }
    private static AccessibilityNodeInfo root() {
        return automation.getRootInActiveWindow();
    }
    private static JSONObject screen() throws Exception {
        AccessibilityNodeInfo active = root();
        if (active == null) throw new Refused("screen_unavailable");
        Object device = invoke("deviceInfo",new JSONArray());
        int width = ((Number)device.getClass().getMethod("getDisplayWidth").invoke(device)).intValue();
        int height = ((Number)device.getClass().getMethod("getDisplayHeight").invoke(device)).intValue();
        String app = active.getPackageName() == null ? "" : active.getPackageName().toString();
        JSONArray elements = new JSONArray(); rows = 0;
        List<AccessibilityWindowInfo> windows = automation.getWindows();
        if (windows == null || windows.isEmpty()) walk(active,0,elements);
        else for (AccessibilityWindowInfo window : windows) {
            AccessibilityNodeInfo windowRoot = Build.VERSION.SDK_INT >= 33 ? window.getRoot(AccessibilityNodeInfo.FLAG_PREFETCH_DESCENDANTS_HYBRID) : window.getRoot();
            if (windowRoot != null) { walk(windowRoot,0,elements); windowRoot.recycle(); }
        }
        active.recycle();
        return new JSONObject().put("device","").put("app",app).put("width",width).put("height",height).put("elements",elements);
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
        for (AccessibilityWindowInfo window : automation.getWindows()) {
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
