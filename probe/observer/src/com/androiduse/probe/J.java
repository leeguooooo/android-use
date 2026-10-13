package com.androiduse.probe;

import org.json.JSONException;
import org.json.JSONObject;

/** JSONObject whose put never throws a checked exception, for use in callbacks. */
final class J extends JSONObject {
    private J set(String key, Object value) {
        try { super.put(key, value); } catch (JSONException e) { throw new IllegalArgumentException(e); }
        return this;
    }
    @Override public J put(String key, Object value) { return set(key, value); }
    @Override public J put(String key, boolean value) { return set(key, value); }
    @Override public J put(String key, int value) { return set(key, value); }
    @Override public J put(String key, long value) { return set(key, value); }
    @Override public J put(String key, double value) { return set(key, value); }
}
