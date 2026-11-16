#!/usr/bin/env bash
# Start, stop, or check the AWS Emulator container for Openbook.
#
#   scripts/emulator.sh up [port]
#   scripts/emulator.sh down [port]
#   scripts/emulator.sh status [port]
#
# The port defaults to $OPENBOOK_EMULATOR_PORT, then 9297. The container is named
# openbook-emulator-<port> and is published on 127.0.0.1 only. Authentication is off
# (`-a off`), which is safe only because of that. Telemetry is off unless you set
# EMULATOR_TELEMETRY=on. State lives in memory, so `down` then `up` gives a clean state.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image="${EMULATOR_IMAGE:-aws-emulator}"
health_timeout="${EMULATOR_HEALTH_TIMEOUT:-90}"

usage() {
  echo "usage: $0 up|down|status [port]" >&2
  exit 2
}

[ $# -ge 1 ] && [ $# -le 2 ] || usage
command="$1"
port="${2:-${OPENBOOK_EMULATOR_PORT:-9297}}"
if [[ ! "$port" =~ ^[0-9]+$ ]] || [ "$port" -lt 1024 ] || [ "$port" -gt 65535 ]; then
  echo "error: the port must be a number from 1024 to 65535, got '$port'" >&2
  exit 2
fi

name="openbook-emulator-${port}"
emulator_dir="${HOME}/.aws/emulator/${port}"
ca_pem="${emulator_dir}/ca.pem"
health_url="https://127.0.0.1:${port}/__admin/health"

require_docker() {
  if ! command -v docker > /dev/null 2>&1; then
    echo "error: docker is not installed" >&2
    exit 1
  fi
  if ! docker info > /dev/null 2>&1; then
    echo "error: the Docker daemon is not running (start Docker Desktop or Colima)" >&2
    exit 1
  fi
}

is_running() {
  [ "$(docker inspect -f '{{.State.Running}}' "$name" 2> /dev/null || true)" = "true" ]
}

is_healthy() {
  [ -s "$ca_pem" ] && curl -fsS --max-time 3 --cacert "$ca_pem" -o /dev/null "$health_url" 2> /dev/null
}

wait_healthy() {
  local waited=0
  while [ "$waited" -lt "$health_timeout" ]; do
    if ! is_running; then
      echo "error: the emulator container stopped. Its log:" >&2
      docker logs --tail 30 "$name" >&2 || true
      return 1
    fi
    if is_healthy; then
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  echo "error: the emulator was not healthy after ${health_timeout}s. Its log:" >&2
  docker logs --tail 30 "$name" >&2 || true
  return 1
}

ensure_image() {
  if ! docker image inspect "$image" > /dev/null 2>&1; then
    echo "Building the emulator image '${image}' from the latest LTS RPM"
    docker build --pull -t "$image" "${repo_root}/scripts/emulator"
  fi
}

case "$command" in
  up)
    require_docker
    if is_running && is_healthy; then
      echo "emulator already running: ${name} on 127.0.0.1:${port}"
      exit 0
    fi
    ensure_image
    docker rm -f "$name" > /dev/null 2>&1 || true
    # The folder must exist before `docker run`, or Docker creates it as root and the
    # emulator (user 1000) cannot write its CA there. Start from a new folder every time:
    # the emulator fails with "Operation not permitted" when the folder already holds the
    # files of an earlier run, and a stale ca.pem must never pass the health check.
    rm -rf "$emulator_dir"
    mkdir -p "$emulator_dir"
    chmod 700 "$emulator_dir"
    # Run as the caller. The folder is mode 700 and owned by the caller (the runner is uid
    # 1001 on GitHub, not the uid 1000 of the image), so the container user must match it.
    docker run -d \
      --name "$name" \
      --user "$(id -u):$(id -g)" \
      --label openbook.emulator=true \
      -p "127.0.0.1:${port}:${port}" \
      -e "EMULATOR_TELEMETRY=${EMULATOR_TELEMETRY:-off}" \
      -v "${emulator_dir}:/home/emulator/.aws/emulator/${port}" \
      "$image" -p "$port" -a off > /dev/null
    if ! wait_healthy; then
      docker rm -f "$name" > /dev/null 2>&1 || true
      exit 1
    fi
    echo "emulator ready: ${name} on https://127.0.0.1:${port} (CA: ${ca_pem})"
    ;;
  down)
    require_docker
    if docker inspect "$name" > /dev/null 2>&1; then
      docker rm -f "$name" > /dev/null
      echo "emulator stopped: ${name}"
    else
      echo "emulator not running: ${name}"
    fi
    ;;
  status)
    require_docker
    if ! docker inspect "$name" > /dev/null 2>&1; then
      echo "emulator not running: ${name}"
      exit 1
    fi
    if is_running && is_healthy; then
      echo "emulator healthy: ${name} on https://127.0.0.1:${port} (CA: ${ca_pem})"
    elif is_running; then
      echo "emulator running but not healthy: ${name}"
      exit 1
    else
      echo "emulator container exists but is stopped: ${name}"
      exit 1
    fi
    ;;
  *)
    usage
    ;;
esac
