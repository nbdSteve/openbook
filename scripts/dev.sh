#!/usr/bin/env bash
# The full local stack: AWS Emulator, bootstrap, the API, and the Vite dev server.
# See docs/local-dev.md. Ctrl-C stops everything this script started.
#
# Settings (environment): OPENBOOK_EMULATOR_PORT (9297), OPENBOOK_API_PORT (8787),
# OPENBOOK_WEB_PORT (5173), EMULATOR_TELEMETRY (off), and OPENBOOK_BEDROCK_PROFILE
# (real Bedrock through that AWS profile; everything else stays on the emulator).
# shellcheck disable=SC2031 # the subshells in scripts/lib/stack.sh export on purpose
# shellcheck disable=SC2329 # cleanup runs from the EXIT trap
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
# shellcheck source=scripts/lib/stack.sh
. scripts/lib/stack.sh

emulator_port="${OPENBOOK_EMULATOR_PORT:-9297}"
api_port="${OPENBOOK_API_PORT:-8787}"
web_port="${OPENBOOK_WEB_PORT:-5173}"
started_emulator=0

cleanup() {
  trap '' INT TERM
  echo
  stack_log "stopping"
  stack_stop 2> /dev/null
  if [ "$started_emulator" -eq 1 ]; then
    scripts/emulator.sh down "$emulator_port" || true
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

stack_port_free "$api_port" "API"
stack_port_free "$web_port" "web"

if scripts/emulator.sh status "$emulator_port" > /dev/null 2>&1; then
  stack_log "using the emulator that is already running on port ${emulator_port}"
else
  scripts/emulator.sh up "$emulator_port"
  started_emulator=1
fi

stack_dir=".local/dev"
scripts/bootstrap-local.sh \
  --emulator-port "$emulator_port" --web-port "$web_port" --api-port "$api_port" \
  --stack-dir "$stack_dir"

if [ -n "${OPENBOOK_BEDROCK_PROFILE:-}" ]; then
  stack_log "chat uses real Bedrock through AWS profile '${OPENBOOK_BEDROCK_PROFILE}'"
else
  stack_log "chat uses the fake model (set OPENBOOK_BEDROCK_PROFILE to use real Bedrock)"
fi

stack_start api stack_run_api "${stack_dir}/api.env"
stack_start ingest stack_run_ingest "${stack_dir}/api.env"
stack_start web stack_run_web "$emulator_port" "$api_port" "$web_port" "${repo_root}/${stack_dir}/web-config.json"

# The first API start compiles the Rust workspace, which can take a few minutes.
stack_wait_url "API" "http://127.0.0.1:${api_port}/api/health" 900
stack_wait_url "web" "http://localhost:${web_port}/config.json" 120

stack_log "ready: http://localhost:${web_port}  (sign in as teacher@openbook.test or student@openbook.test, password Openbook-local-1)"
stack_log "Ctrl-C stops everything"

while stack_alive; do
  sleep 1
done
echo "error: a process exited. Stopping the stack." >&2
exit 1
