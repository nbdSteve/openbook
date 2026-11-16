#!/usr/bin/env bash
# Downloads the pdfium library that openbook-ingest needs, and verifies its SHA-256.
#
# Usage: scripts/fetch-pdfium.sh [mac-arm64|linux-x64|linux-arm64]
# Without an argument it uses the platform of this machine.
# The files go to .local/pdfium/<platform>/ (lib/ has the library). The script is idempotent:
# it downloads nothing when the verified library is already there. It prints the library
# directory on the last line of standard output.
set -euo pipefail

# Release of bblanchon/pdfium-binaries (Chromium build 8152). Do not change one value without the others.
RELEASE="chromium/8152"
SHA256_MAC_ARM64="30b0c3f55448840f0947b54bedbe35185bbaccbadd35b8cd960270d6fa6caf80"
SHA256_LINUX_X64="384ab5c51882408a143a21f9c14dcd2caf1f75280ae41c554d0489b46c3dba9f"
SHA256_LINUX_ARM64="c6ed1a91234aede0cfafdc09c401b83bd44ac69deafc4599fda75d511c36403e"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

platform="${1:-}"
if [ -z "$platform" ]; then
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) platform="mac-arm64" ;;
    Linux-x86_64) platform="linux-x64" ;;
    Linux-aarch64 | Linux-arm64) platform="linux-arm64" ;;
    *) echo "Unsupported platform $(uname -s)-$(uname -m). Pass mac-arm64, linux-x64, or linux-arm64." >&2; exit 1 ;;
  esac
fi

case "$platform" in
  mac-arm64) expected="$SHA256_MAC_ARM64"; library="libpdfium.dylib" ;;
  linux-x64) expected="$SHA256_LINUX_X64"; library="libpdfium.so" ;;
  linux-arm64) expected="$SHA256_LINUX_ARM64"; library="libpdfium.so" ;;
  *) echo "Unknown platform $platform. Use mac-arm64, linux-x64, or linux-arm64." >&2; exit 1 ;;
esac

dest="$root/.local/pdfium/$platform"
marker="$dest/.sha256"

if [ -f "$dest/lib/$library" ] && [ -f "$marker" ] && [ "$(cat "$marker")" = "$expected" ]; then
  echo "$dest/lib"
  exit 0
fi

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
archive="$tmp/pdfium-$platform.tgz"
url="https://github.com/bblanchon/pdfium-binaries/releases/download/${RELEASE//\//%2F}/pdfium-$platform.tgz"

echo "Downloading pdfium $RELEASE for $platform" >&2
curl --fail --silent --show-error --location --retry 3 --output "$archive" "$url"

actual="$(sha256_of "$archive")"
if [ "$actual" != "$expected" ]; then
  echo "SHA-256 mismatch for $url" >&2
  echo "  expected $expected" >&2
  echo "  actual   $actual" >&2
  exit 1
fi

rm -rf "$dest"
mkdir -p "$dest"
tar -xzf "$archive" -C "$dest"
if [ ! -f "$dest/lib/$library" ]; then
  echo "The archive has no lib/$library" >&2
  exit 1
fi
echo "$expected" > "$marker"
echo "$dest/lib"
