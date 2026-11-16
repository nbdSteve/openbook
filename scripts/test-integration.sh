#!/usr/bin/env bash
# Rust integration tests against AWS Emulator.
#
# Starts an emulator on port 9320 (OPENBOOK_TEST_EMULATOR_PORT), runs the ignored tests of the
# workspace against it, and stops the emulator, even when the tests fail. The exit code is the
# test result.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# shellcheck source=scripts/lib/pdfium.sh
. scripts/lib/pdfium.sh

port="${OPENBOOK_TEST_EMULATOR_PORT:-9320}"

cleanup() {
  scripts/emulator.sh down "$port" || true
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# The ingest tests open PDFs with pdfium. Fetch the pinned build for this machine first, and
# let the tests find it.
pdfium_path=""
if [ -x "${OPENBOOK_PDFIUM_FETCH:-scripts/fetch-pdfium.sh}" ]; then
  pdfium_path="$(pdfium_fetch "$(pdfium_platform)")"
else
  echo "warning: scripts/fetch-pdfium.sh does not exist yet, so OPENBOOK_PDFIUM_PATH is not set" >&2
fi

# A fresh emulator, so that no test depends on the state of an earlier run.
scripts/emulator.sh down "$port"
scripts/emulator.sh up "$port"

# The tests talk to the emulator with test keys only. Drop every real AWS variable first.
while IFS= read -r name; do
  unset "$name"
done < <(env | sed -n 's/^\(AWS_[A-Za-z0-9_]*\)=.*/\1/p')

if [ -n "$pdfium_path" ]; then export OPENBOOK_PDFIUM_PATH="$pdfium_path"; fi
export OPENBOOK_TEST_EMULATOR_ENDPOINT="https://127.0.0.1:${port}"
export OPENBOOK_TEST_EMULATOR_CA="${HOME}/.aws/emulator/${port}/ca.pem"

cargo test --workspace -- --ignored
