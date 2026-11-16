#!/usr/bin/env bash
# Helpers for pdfium. Source this file; do not run it.
#
# `scripts/fetch-pdfium.sh <platform>` downloads the pinned pdfium build into
# .local/pdfium/<platform>/ and prints the library path. The platform names are the ones of
# bblanchon/pdfium-binaries: linux-x64, linux-arm64, mac-x64, mac-arm64.

# pdfium_platform: the platform of this machine.
pdfium_platform() {
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) echo linux-x64 ;;
    Linux-aarch64 | Linux-arm64) echo linux-arm64 ;;
    Darwin-arm64) echo mac-arm64 ;;
    Darwin-x86_64) echo mac-x64 ;;
    *)
      echo "error: no pdfium build for $(uname -s) $(uname -m)" >&2
      return 1
      ;;
  esac
}

# pdfium_fetch <platform>: run the fetch script and print what it prints (the library path).
# OPENBOOK_PDFIUM_FETCH overrides the script, for a test of the callers.
pdfium_fetch() {
  local fetch="${OPENBOOK_PDFIUM_FETCH:-${repo_root}/scripts/fetch-pdfium.sh}"
  if [ ! -x "$fetch" ]; then
    echo "error: ${fetch} does not exist or is not executable" >&2
    return 1
  fi
  "$fetch" "$1"
}

# pdfium_library_file <path>: the library file for the path that the fetch script printed. The
# path is the file itself, or a folder that holds it (as in OPENBOOK_PDFIUM_PATH).
pdfium_library_file() {
  local path="$1" candidate
  if [ -f "$path" ]; then
    echo "$path"
    return 0
  fi
  for candidate in "$path/libpdfium.so" "$path/libpdfium.dylib" "$path/lib/libpdfium.so" "$path/lib/libpdfium.dylib"; do
    if [ -f "$candidate" ]; then
      echo "$candidate"
      return 0
    fi
  done
  echo "error: no libpdfium in ${path}" >&2
  return 1
}
