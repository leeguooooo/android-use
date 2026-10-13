package com.androiduse.stub;

import android.accessibilityservice.AccessibilityService;
import android.view.accessibility.AccessibilityEvent;

/** Does nothing; exists so enabling and self-restoring a service can be tested. */
public class StubService extends AccessibilityService {
    @Override public void onAccessibilityEvent(AccessibilityEvent event) {}
    @Override public void onInterrupt() {}
}
