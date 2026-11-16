#!/usr/bin/env bash
# Rust integration tests against AWS Emulator.
#
# Starts an emulator on port 9320 (OPENBOOK_TEST_EMULATOR_PORT), runs the ignored tests of the
# workspace against it, and stops the emulator, even when the tests fail. The exit code is the
# test result.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

port="${OPENBOOK_TEST_EMULATOR_PORT:-9320}"

cleanup() {
  scripts/emulator.sh down "$port" || true
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# A fresh emulator, so that no test depends on the state of an earlier run.
scripts/emulator.sh down "$port"
scripts/emulator.sh up "$port"

# The tests talk to the emulator with test keys only. Drop every real AWS variable first.
while IFS= read -r name; do
  unset "$name"
done < <(env | sed -n 's/^\(AWS_[A-Za-z0-9_]*\)=.*/\1/p')

export OPENBOOK_TEST_EMULATOR_ENDPOINT="https://127.0.0.1:${port}"
export OPENBOOK_TEST_EMULATOR_CA="${HOME}/.aws/emulator/${port}/ca.pem"

cargo test --workspace -- --ignored
