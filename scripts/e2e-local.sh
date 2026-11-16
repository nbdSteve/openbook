#!/usr/bin/env bash
# The local end-to-end suite: a fresh, isolated stack (emulator 9310, API 8790, web 5180),
# the Playwright `flows` project, then a full stop even when the tests fail. The exit code
# is the test result.
#
# The ports and the files (.local/e2e) differ from `make dev` (.local/dev), so both can run
# at once.
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
stack_dir=".local/e2e"

cleanup() {
  trap '' INT TERM
  stack_stop 2> /dev/null
  scripts/emulator.sh down "$emulator_port" || true
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

stack_port_free "$api_port" "API"
stack_port_free "$web_port" "web"

# A fresh emulator every time, so no test depends on an earlier run.
scripts/emulator.sh down "$emulator_port"
scripts/emulator.sh up "$emulator_port"

# The e2e run never uses real Bedrock, even when the caller has the variable set.
OPENBOOK_BEDROCK_PROFILE="" scripts/bootstrap-local.sh \
  --emulator-port "$emulator_port" --web-port "$web_port" --api-port "$api_port" \
  --stack-dir "$stack_dir" --daily-messages-per-user 8

stack_start api stack_run_api "${stack_dir}/api.env"
stack_start web stack_run_web "$emulator_port" "$api_port" "$web_port" "${repo_root}/${stack_dir}/web-config.json"

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
