# Pre-release hardware checks

2026-10-13: Samsung SM-A155M, Android 16, USB. Local native adapter build; these records are not published-binary acceptance. Synthetic native and offline WebView fields passed 8/8 input checks. Ten warm counter tasks passed an independently read postcondition. Median screen reads were 55.07 ms; tap plus semantic wait and settled observation was 676.51 ms. Input checks measured acknowledged/verified dispatch separately from settling. No model was run in the fixed benchmark script, so model round trips are null.

Earlier attempts exposed two useful failures: a changed screen rejected a tap as stale before dispatch, and WebView initially acknowledged text before accessibility updated. The latter produced unknown with no replay; a fresh read confirmed the text. The adapter now waits up to 400 ms for read-only text verification and the same test passes. The counter's displayed uppercase label was also corrected in the benchmark driver.

An iPhone 17 Pro Max/iOS 27 connection briefly became drivable over USB and one screen read measured 210.54 ms. Later navigation returned HTTP 502 and a read timed out; transport changed to a Wi-Fi tunnel and the service became unavailable. This is not a valid cross-device comparison. Do not use these records to claim iphone-use parity or version-to-version acceleration.
