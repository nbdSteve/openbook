#!/usr/bin/env bash
# The local end-to-end suite: a fresh, isolated stack (emulator 9310, API 8790, web 5180),
# the Playwright `flows` project, then a full stop even when the tests fail. The exit code
# is the test result.
#
# The ports differ from `make dev`, so both can run at once. One file is shared: the web
# runtime config at apps/ask/public/config.json. This script saves the file first and puts
# it back at the end, so a running `make dev` keeps its own file afterwards.
# shellcheck disable=SC2031 # the subshells in scripts/lib/stack.sh export on purpose
# shellcheck disable=SC2329 # cleanup runs from the EXIT trap
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
# shellcheck source=scripts/lib/stack.sh
. scripts/lib/stack.sh

# Fixed by design, so that the Playwright config and CI agree. The variables exist so that
# a test of this script itself can use other ports.
emulator_port="${OPENBOOK_E2E_EMULATOR_PORT:-9310}"
api_port="${OPENBOOK_E2E_API_PORT:-8790}"
web_port="${OPENBOOK_E2E_WEB_PORT:-5180}"
web_config="apps/ask/public/config.json"
saved_config=""
had_config=0

cleanup() {
  trap '' INT TERM
  stack_stop 2> /dev/null
  scripts/emulator.sh down "$emulator_port" || true
  if [ "$had_config" -eq 1 ]; then
    cp "$saved_config" "$web_config"
  else
    rm -f "$web_config"
  fi
  if [ -n "$saved_config" ]; then rm -f "$saved_config"; fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

stack_port_free "$api_port" "API"
stack_port_free "$web_port" "web"

saved_config="$(mktemp)"
if [ -f "$web_config" ]; then
  cp "$web_config" "$saved_config"
  had_config=1
fi

# A fresh emulator every time, so no test depends on an earlier run.
scripts/emulator.sh down "$emulator_port"
scripts/emulator.sh up "$emulator_port"

# The e2e run never uses real Bedrock, even when the caller has the variable set.
OPENBOOK_BEDROCK_PROFILE="" scripts/bootstrap-local.sh \
  --emulator-port "$emulator_port" --web-port "$web_port" --api-port "$api_port" \
  --env-file .local/e2e/api.env --web-config "$web_config"

stack_start api stack_run_api .local/e2e/api.env
stack_start web stack_run_web "$emulator_port" "$api_port" "$web_port"

stack_wait_url "API" "http://127.0.0.1:${api_port}/api/health" 900
stack_wait_url "web" "http://localhost:${web_port}/config.json" 120
stack_wait_url "web proxy to the API" "http://localhost:${web_port}/api/health" 30

status=0
(
  export E2E_BASE_URL="http://localhost:${web_port}"
  export E2E_AWS_ENDPOINT="https://127.0.0.1:${emulator_port}"
  export NODE_EXTRA_CA_CERTS="${HOME}/.aws/emulator/${emulator_port}/ca.pem"
  npm --prefix apps/ask run e2e -- --project=flows
) || status=$?

if [ "$status" -ne 0 ]; then
  echo "error: the end-to-end tests failed (exit ${status})" >&2
fi
exit "$status"
