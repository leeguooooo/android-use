#!/bin/sh
set -eu
root=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
sdk=${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}
[ -n "$sdk" ] || { echo 'Set ANDROID_SDK_ROOT' >&2; exit 1; }
ndk=${ANDROID_NDK_HOME:-$sdk/ndk/28.2.13676358}
platform="$sdk/platforms/android-36/android.jar"
tools="$sdk/build-tools/36.0.0"
[ -f "$platform" ] && [ -x "$tools/aapt2" ] || { echo 'SDK 36 / build-tools 36 required' >&2; exit 1; }
case $(uname -s) in Darwin) host=darwin-x86_64 ;; Linux) host=linux-x86_64 ;; *) echo 'Build APK on macOS/Linux' >&2; exit 1 ;; esac
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$ndk/toolchains/llvm/prebuilt/$host/bin/aarch64-linux-android26-clang"
export CC_aarch64_linux_android="$CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"
export AR_aarch64_linux_android="$ndk/toolchains/llvm/prebuilt/$host/bin/llvm-ar"
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
cd "$root"
cargo_tool=${CARGO_TOOL:-$HOME/.local/bin/heavy-cargo}
[ -x "$cargo_tool" ] || cargo_tool=cargo
"$cargo_tool" build -p android-use-phone --release --target aarch64-linux-android --locked
out="$root/phone/build"
rm -rf "$out/classes"
mkdir -p "$out/classes" "$out/lib/arm64-v8a"
"$tools/aapt2" compile --dir "$root/phone/app/res" -o "$out/res.zip"
"$tools/aapt2" link -o "$out/unsigned.apk" -I "$platform" --manifest "$root/phone/app/AndroidManifest.xml" "$out/res.zip"
find "$root/phone/app/src" -name '*.java' > "$out/sources.txt"
javac --release 8 -cp "$platform" -d "$out/classes" @"$out/sources.txt"
java -Xmx384m -cp "$tools/lib/d8.jar" com.android.tools.r8.D8 --min-api 26 --lib "$platform" --output "$out" $(find "$out/classes" -name '*.class')
cp target/aarch64-linux-android/release/libandroid_use_phone.so "$out/lib/arm64-v8a/"
(cd "$out" && zip -q unsigned.apk classes.dex lib/arm64-v8a/libandroid_use_phone.so)
"$tools/zipalign" -f -P 16 4 "$out/unsigned.apk" "$out/aligned.apk"
keystore=${PHONE_KEYSTORE:-$HOME/.android-use/phone-debug.keystore}
if [ ! -f "$keystore" ]; then
  [ -z "${PHONE_KEYSTORE:-}" ] || { echo 'Release keystore missing' >&2; exit 1; }
  mkdir -p "$(dirname "$keystore")"
  keytool -genkeypair -keystore "$keystore" -storepass android -keypass android -alias phone -keyalg RSA -keysize 2048 -validity 3650 -dname 'CN=android-use local development' >/dev/null
fi
# CI passes secrets via environment, not process arguments.
export PHONE_STORE_PASS=${PHONE_STORE_PASS:-android}
export PHONE_KEY_PASS=${PHONE_KEY_PASS:-$PHONE_STORE_PASS}
"$tools/apksigner" sign --ks "$keystore" --ks-pass env:PHONE_STORE_PASS --key-pass env:PHONE_KEY_PASS --out "$out/android-use-phone-arm64.apk" "$out/aligned.apk"
"$tools/apksigner" verify "$out/android-use-phone-arm64.apk"
python3 - <<'PY'
import hashlib,pathlib
p=pathlib.Path('phone/build/android-use-phone-arm64.apk')
p.with_name(p.name+'.sha256').write_text(hashlib.sha256(p.read_bytes()).hexdigest()+'  '+p.name+'\n')
PY
