#!/usr/bin/env bash
# Build both Lambda functions for arm64, and put libpdfium.so next to the ingest bootstrap:
#
#   target/lambda/openbook-api/bootstrap
#   target/lambda/openbook-ingest/bootstrap
#   target/lambda/openbook-ingest/libpdfium.so
#
# The CDK app refuses an ingest asset directory without libpdfium.so.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
# shellcheck source=scripts/lib/pdfium.sh
. scripts/lib/pdfium.sh

if ! command -v cargo-lambda > /dev/null 2>&1; then
  echo "error: cargo-lambda is not installed (see https://www.cargo-lambda.info)" >&2
  exit 1
fi

cargo lambda build --release --arm64 -p openbook-api -p openbook-ingest

printed="$(pdfium_fetch linux-arm64)"
library="$(pdfium_library_file "$printed")"
cp -f "$library" target/lambda/openbook-ingest/libpdfium.so

for file in target/lambda/openbook-api/bootstrap target/lambda/openbook-ingest/bootstrap; do
  if [ ! -x "$file" ]; then
    echo "error: ${file} is missing or is not executable" >&2
    exit 1
  fi
done
if [ ! -s target/lambda/openbook-ingest/libpdfium.so ]; then
  echo "error: target/lambda/openbook-ingest/libpdfium.so is empty" >&2
  exit 1
fi
echo "built: openbook-api, openbook-ingest, and libpdfium.so (linux-arm64)"
