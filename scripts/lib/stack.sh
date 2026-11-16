#!/usr/bin/env bash
# Shared helpers for scripts/dev.sh and scripts/e2e-local.sh. Source this file; do not run it.
# Works with the Bash 3.2 that macOS ships.

# Job control gives every background job its own process group, so one `kill -- -PID`
# stops a whole pipeline (the program and its log prefixer) and every child it started.
set -m

stack_pids=()

stack_log() { printf '[stack] %s\n' "$*"; }

# stack_start <label> <command...>: run a command in the background with every output line
# prefixed by "[label]". The command runs in the current environment.
stack_start() {
  local label="$1"
  shift
  { "$@" 2>&1 | awk -v p="[${label}] " '{ print p $0; fflush() }'; } &
  stack_pids+=("$!")
}

# stack_alive: true while every started job is still running.
stack_alive() {
  local pid
  for pid in ${stack_pids[@]+"${stack_pids[@]}"}; do
    kill -0 "$pid" 2> /dev/null || return 1
  done
  return 0
}

# stack_stop: stop every started job and its children. Safe to call twice.
stack_stop() {
  local pid
  for pid in ${stack_pids[@]+"${stack_pids[@]}"}; do
    kill -TERM -- "-${pid}" 2> /dev/null || kill -TERM "$pid" 2> /dev/null || true
  done
  local waited=0
  while [ "$waited" -lt 20 ]; do
    local any=0
    for pid in ${stack_pids[@]+"${stack_pids[@]}"}; do
      if kill -0 "$pid" 2> /dev/null; then any=1; fi
    done
    [ "$any" -eq 0 ] && break
    sleep 0.5
    waited=$((waited + 1))
  done
  for pid in ${stack_pids[@]+"${stack_pids[@]}"}; do
    kill -KILL -- "-${pid}" 2> /dev/null || true
    # Reap the job here, so that the shell does not print "Terminated" notices later.
    { wait "$pid"; } 2> /dev/null || true
  done
  stack_pids=()
}

# stack_port_free <port>: fail with a message when something already listens on the port.
stack_port_free() {
  local port="$1" what="$2"
  if command -v lsof > /dev/null 2>&1 && lsof -nP -iTCP:"$port" -sTCP:LISTEN > /dev/null 2>&1; then
    echo "error: port ${port} (${what}) is already in use" >&2
    return 1
  fi
}

# stack_wait_url <label> <url> <timeout-seconds>: wait until the URL answers with a 2xx
# status. Stops waiting early (and fails) when a started job has exited.
stack_wait_url() {
  local label="$1" url="$2" timeout="$3" waited=0
  while [ "$waited" -lt "$timeout" ]; do
    if curl -fsS --max-time 3 -o /dev/null "$url" 2> /dev/null; then
      stack_log "${label} is ready: ${url}"
      return 0
    fi
    if ! stack_alive; then
      echo "error: a process exited while waiting for ${label}" >&2
      return 1
    fi
    sleep 1
    waited=$((waited + 1))
  done
  echo "error: ${label} was not ready after ${timeout}s (${url})" >&2
  return 1
}

# stack_run_api <env-file> <port>: run the API with only the variables of the env file.
# Every AWS_* variable of the caller is dropped, so a real AWS profile cannot leak into
# the local API.
stack_run_api() {
  local env_file="$1"
  (
    while IFS= read -r name; do
      unset "$name"
    done < <(env | sed -n 's/^\(AWS_[A-Za-z0-9_]*\)=.*/\1/p')
    set -a
    # shellcheck disable=SC1090 # the file is written by scripts/bootstrap-local.sh
    . "$env_file"
    set +a
    exec cargo run --quiet -p openbook-api
  )
}

# stack_run_web <emulator-port> <api-port> <web-port>: run the Vite dev server.
stack_run_web() {
  local emulator_port="$1" api_port="$2" web_port="$3"
  (
    export NODE_EXTRA_CA_CERTS="${HOME}/.aws/emulator/${emulator_port}/ca.pem"
    export OPENBOOK_EMULATOR_PORT="$emulator_port"
    export OPENBOOK_API_PORT="$api_port"
    export OPENBOOK_WEB_PORT="$web_port"
    exec npm --prefix apps/ask run dev
  )
}
