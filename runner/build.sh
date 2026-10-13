#!/bin/sh
set -eu
# Developer/CI only. End users receive this adapter embedded in the Rust CLI.
root=$(CDPATH='' cd -- "$(dirname "$0")" && pwd)
android_sdk=${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}
[ -n "$android_sdk" ] || { echo 'Set ANDROID_SDK_ROOT with Android platform 36 and build tools 36.0.0' >&2; exit 1; }
platform="$android_sdk/platforms/android-36/android.jar"
d8="$android_sdk/build-tools/36.0.0/lib/d8.jar"
[ -f "$platform" ] && [ -f "$d8" ] || { echo 'Android platform/build tools 36 missing' >&2; exit 1; }
mkdir -p "$root/build/classes"
javac --release 8 -cp "$platform" -d "$root/build/classes" "$root/src/com/androiduse/Runner.java"
java -Xmx384m -cp "$d8" com.android.tools.r8.D8 --min-api 26 --lib "$platform" --output "$root/build" "$root"/build/classes/com/androiduse/Runner*.class
cp "$root/build/classes.dex" "$root/adapter.dex"
python3 - "$root" <<'PY'
import pathlib,hashlib,json,sys
p=pathlib.Path(sys.argv[1]);m=json.loads((p/'manifest.json').read_text())
m['source_sha256']=hashlib.sha256((p/'src/com/androiduse/Runner.java').read_bytes()).hexdigest()
m['adapter_sha256']=hashlib.sha256((p/'adapter.dex').read_bytes()).hexdigest()
(p/'manifest.json').write_text(json.dumps(m,indent=2)+'\n')
PY
