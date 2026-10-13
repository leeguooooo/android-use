#!/bin/sh
set -eu
# Developer only. Builds observer, stub service and native/WebView fixture APKs.
# Neither ships in releases. The debug keystore stays outside the repo.
root=$(CDPATH='' cd -- "$(dirname "$0")" && pwd)
sdk=${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}
[ -n "$sdk" ] || { echo 'Set ANDROID_SDK_ROOT with Android platform 36 and build tools 36.0.0' >&2; exit 1; }
platform="$sdk/platforms/android-36/android.jar"
tools="$sdk/build-tools/36.0.0"
[ -f "$platform" ] && [ -x "$tools/aapt2" ] || { echo 'Android platform/build tools 36 missing' >&2; exit 1; }
keystore=${PROBE_KEYSTORE:-$HOME/.android-use/probe-debug.keystore}
if [ ! -f "$keystore" ]; then
    mkdir -p "$(dirname "$keystore")"
    keytool -genkeypair -keystore "$keystore" -storepass android -keypass android \
        -alias probe -keyalg RSA -keysize 2048 -validity 3650 -dname 'CN=android-use probe' >/dev/null
fi
for app in observer stub fixture; do
    src="$root/$app"; out="$root/build/$app"
    rm -rf "$out"; mkdir -p "$out/classes" "$out/res"
    set --
    if [ -d "$src/res" ]; then
        "$tools/aapt2" compile --dir "$src/res" -o "$out/res.zip"
        set -- "$out/res.zip"
    fi
    "$tools/aapt2" link -o "$out/unsigned.apk" -I "$platform" --manifest "$src/AndroidManifest.xml" "$@"
    find "$src/src" -name '*.java' > "$out/sources.txt"
    javac --release 8 -cp "$platform" -d "$out/classes" @"$out/sources.txt"
    java -Xmx384m -cp "$tools/lib/d8.jar" com.android.tools.r8.D8 --min-api 26 --lib "$platform" \
        --output "$out" $(find "$out/classes" -name '*.class')
    (cd "$out" && zip -qj unsigned.apk classes.dex)
    "$tools/zipalign" -f 4 "$out/unsigned.apk" "$out/aligned.apk"
    "$tools/apksigner" sign --ks "$keystore" --ks-pass pass:android --key-pass pass:android \
        --out "$root/build/$app.apk" "$out/aligned.apk"
done
ls -l "$root/build/observer.apk" "$root/build/stub.apk" "$root/build/fixture.apk"
