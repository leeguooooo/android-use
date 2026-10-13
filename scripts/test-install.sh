#!/bin/sh
set -eu
repo=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
android_install_tmp=$(mktemp -d)
trap 'rm -rf "$android_install_tmp"' EXIT HUP INT TERM
mkdir -p "$android_install_tmp/mock" "$android_install_tmp/package" "$android_install_tmp/install"
cat > "$android_install_tmp/package/android-use" <<'SH'
#!/bin/sh
echo 'android-use 0.1.0'
SH
chmod +x "$android_install_tmp/package/android-use"
tar -czf "$android_install_tmp/archive.tar.gz" -C "$android_install_tmp/package" .
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$android_install_tmp/archive.tar.gz" > "$android_install_tmp/archive.sha256"
else
    shasum -a 256 "$android_install_tmp/archive.tar.gz" > "$android_install_tmp/archive.sha256"
fi
cat > "$android_install_tmp/mock/uname" <<'SH'
#!/bin/sh
case "$1" in -s) echo Linux ;; -m) echo x86_64 ;; esac
SH
cat > "$android_install_tmp/mock/curl" <<'SH'
#!/bin/sh
url=''
while [ "$#" -gt 0 ]; do
    case "$1" in -o) shift; output=$1 ;; https:*) url=$1 ;; esac
    shift
done
case "$url" in
    *.sha256) cp "$ANDROID_INSTALL_FIXTURE/archive.sha256" "$output" ;;
    *.tar.gz) cp "$ANDROID_INSTALL_FIXTURE/archive.tar.gz" "$output" ;;
    *) exit 1 ;;
esac
SH
chmod +x "$android_install_tmp/mock/curl" "$android_install_tmp/mock/uname"
export ANDROID_INSTALL_FIXTURE="$android_install_tmp"
export ANDROID_USE_TAG=v0.1.0 ANDROID_USE_INSTALL_DIR="$android_install_tmp/install"
export PATH="$android_install_tmp/mock:$PATH"
sh "$repo/install.sh"
test "$("$ANDROID_USE_INSTALL_DIR/android-use" --version)" = 'android-use 0.1.0'
printf 'old binary\n' > "$ANDROID_USE_INSTALL_DIR/android-use"
printf 'invalid checksum\n' > "$android_install_tmp/archive.sha256"
if sh "$repo/install.sh" > /dev/null 2>&1; then
    echo 'Installer accepted a bad checksum' >&2; exit 1
fi
test "$(cat "$ANDROID_USE_INSTALL_DIR/android-use")" = 'old binary'
echo 'Android installer: valid archive installs; checksum failure preserves existing binary.'
