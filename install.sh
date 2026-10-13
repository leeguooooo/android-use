#!/bin/sh
set -eu

# Install an independently versioned android-use release.
tag=${ANDROID_USE_TAG:-}
if [ -z "$tag" ]; then
    latest=$(curl -fsSL -o /dev/null -w '%{url_effective}' https://github.com/leeguooooo/android-use/releases/latest)
    tag=${latest##*/}
fi
case "$tag" in v[0-9]* ) ;; * ) echo 'ANDROID_USE_TAG must be a v-prefixed version' >&2; exit 1 ;; esac
case "$tag" in *[!a-zA-Z0-9._-]* ) echo 'Invalid release tag' >&2; exit 1 ;; esac
case "$(uname -s)" in Darwin) platform=darwin ;; Linux) platform=linux ;; *) echo 'Download the Windows archive from GitHub Releases.' >&2; exit 1 ;; esac
case "$(uname -m)" in arm64|aarch64) arch=arm64 ;; x86_64|amd64) arch=x86_64 ;; *) echo 'Unsupported CPU architecture' >&2; exit 1 ;; esac
if [ "$platform-$arch" = linux-arm64 ]; then echo 'Linux ARM64 binaries are not available in v0.1.' >&2; exit 1; fi
asset="android-use-$platform-$arch.tar.gz"
base="https://github.com/leeguooooo/android-use/releases/download/$tag"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
curl -fLsS "$base/$asset" -o "$work/$asset"
curl -fLsS "$base/$asset.sha256" -o "$work/$asset.sha256"
expected=$(awk 'NR == 1 {print $1}' "$work/$asset.sha256")
if command -v sha256sum >/dev/null 2>&1; then actual=$(sha256sum "$work/$asset" | awk '{print $1}');
else actual=$(shasum -a 256 "$work/$asset" | awk '{print $1}'); fi
if [ "$expected" != "$actual" ]; then echo 'Android archive checksum mismatch' >&2; exit 1; fi
mkdir "$work/unpacked"
tar -xzf "$work/$asset" -C "$work/unpacked"
"$work/unpacked/android-use" --version
destination=${ANDROID_USE_INSTALL_DIR:-"$HOME/.local/bin"}
mkdir -p "$destination"
temporary="$destination/.android-use-install-$$"
cp "$work/unpacked/android-use" "$temporary"
chmod 755 "$temporary"
mv "$temporary" "$destination/android-use"
echo "Installed $destination/android-use. Run android-use setup, then android-use connect (or connect --serial DEVICE_SERIAL)."
