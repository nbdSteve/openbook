#!/usr/bin/env bash
# Shared helpers for scripts/dev.sh and scripts/e2e-local.sh. Source this file; do not run it.
# Works with the Bash 3.2 that macOS ships.

# Job control gives every background job its own process group, so one `kill -- -PID`
# stops a whole pipeline (the program and its log prefixer) and every child it started.
set -m

stack_pids=()

# Each started program also writes its raw output (no prefix) to $STACK_LOG_DIR/<label>.log,
# so that a script can wait for a line of the log (see stack_wait_log).
STACK_LOG_DIR="${STACK_LOG_DIR:-${TMPDIR:-/tmp}/openbook-stack-$$}"
mkdir -p "$STACK_LOG_DIR"

stack_log() { printf '[stack] %s\n' "$*"; }

# stack_start <label> <command...>: run a command in the background with every output line
# prefixed by "[label]". The command runs in the current environment.
stack_start() {
  local label="$1"
  shift
  # Job control gives a background job the terminal. A program that reads stdin (Vite does)
  # would then get SIGTTIN and freeze when the user presses a key. Give it /dev/null instead.
  : > "${STACK_LOG_DIR}/${label}.log"
  { "$@" < /dev/null 2>&1 | tee -a "${STACK_LOG_DIR}/${label}.log" | awk -v p="[${label}] " '{ print p $0; fflush() }'; } &
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
    # A stopped job (SIGTTIN, SIGTSTP) does not act on SIGTERM until it continues.
    kill -CONT -- "-${pid}" 2> /dev/null || true
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

# stack_binary <name>: the path of a binary that stack_build made. It reads the target folder
# from cargo, so CARGO_TARGET_DIR and build.target-dir are respected.
stack_binary() {
  local target
  target="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)"
  echo "${target}/debug/$1"
}

# stack_run_bin <env-file> <binary> [args...]: run a built binary with only the variables of
# the env file plus a short allowlist. Nothing else of the caller's environment gets through,
# so an exported OPENBOOK_LIMIT_* value, an AWS profile, or a real credential cannot change
# the local API or worker.
#
# It runs the binary that `stack_build` made, never `cargo run`. `cargo build -p a -p b` builds
# both programs with one set of unified features, and `cargo run -p a` alone would use another
# set, so it would compile again, silently, while the scripts wait.
stack_run_bin() {
  local env_file="$1" binary="$2" entry name
  shift 2
  local allowed=()
  while IFS= read -r -d '' entry; do
    name="${entry%%=*}"
    case "$name" in
      PATH | HOME | USER | LOGNAME | TERM | TMPDIR | LANG | LC_* | XDG_*)
        allowed+=("$entry")
        ;;
    esac
  done < <(env -0)
  # shellcheck disable=SC2016 # $1 and $@ are for the inner bash
  exec env -i ${allowed[@]+"${allowed[@]}"} bash -c 'set -a; . "$1"; set +a; shift; exec "$@"' _ "$env_file" "$binary" "$@"
}

# stack_run_api <env-file>: the API.
stack_run_api() {
  stack_run_bin "$1" "$(stack_binary openbook-api)"
}

# stack_run_ingest <env-file>: the ingest worker, polling the local queue.
stack_run_ingest() {
  stack_run_bin "$1" "$(stack_binary openbook-ingest)" --poll
}

# stack_run_web <emulator-port> <api-port> <web-port> <web-config-file>: run the Vite dev server.
# The dev server serves the web config file as /config.json.
stack_run_web() {
  local emulator_port="$1" api_port="$2" web_port="$3" web_config="$4"
  (
    export NODE_EXTRA_CA_CERTS="${HOME}/.aws/emulator/${emulator_port}/ca.pem"
    export OPENBOOK_EMULATOR_PORT="$emulator_port"
    export OPENBOOK_API_PORT="$api_port"
    export OPENBOOK_WEB_PORT="$web_port"
    export OPENBOOK_WEB_CONFIG="$web_config"
    exec npm --prefix apps/ask run dev
  )
}

# stack_build: build the API and the worker before anything starts. The first build of a cold
# cache compiles hundreds of crates. Without this step, `cargo run --quiet` compiles them
# silently while the scripts wait, and the wait times out. The output stays visible, so a CI
# log shows the progress. After it, the scripts start the built binaries directly.
stack_build() {
  stack_log "building openbook-api and openbook-ingest"
  cargo build -p openbook-api -p openbook-ingest
}

# stack_wait_log <label> <text> <timeout-seconds>: wait until the log of a started program
# has a line that contains the text. Stops early (and fails) when a started job has exited.
stack_wait_log() {
  local label="$1" text="$2" timeout="$3" waited=0
  while [ "$waited" -lt "$timeout" ]; do
    if grep -qF -- "$text" "${STACK_LOG_DIR}/${label}.log" 2> /dev/null; then
      stack_log "${label} is ready: \"${text}\""
      return 0
    fi
    if ! stack_alive; then
      echo "error: a process exited while waiting for ${label}" >&2
      return 1
    fi
    sleep 1
    waited=$((waited + 1))
  done
  echo "error: ${label} did not log \"${text}\" within ${timeout}s" >&2
  return 1
}
