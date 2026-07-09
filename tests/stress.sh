#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(dirname "$(dirname "$(readlink -f "$0")")")"
SERVER_BIN="$ROOT_DIR/target/debug/localhost"
CONFIG_PATH="$ROOT_DIR/config/default.conf"
SERVER_PID=""

cleanup() {
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}

trap cleanup EXIT

cd "$ROOT_DIR"
cargo build >/dev/null

"$SERVER_BIN" "$CONFIG_PATH" >/tmp/opencode/localhost-stress.log 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 50); do
    if curl -fsS http://127.0.0.1:8080/ >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done

if command -v siege >/dev/null 2>&1; then
    siege -b -t10S -c25 http://127.0.0.1:8080/ http://127.0.0.1:8080/listing/ http://127.0.0.1:8080/cgi/echo.py?source=stress
else
    failures=0
    for _ in $(seq 1 500); do
        curl -fsS http://127.0.0.1:8080/ >/dev/null || failures=$((failures + 1))
        curl -fsS http://127.0.0.1:8080/listing/ >/dev/null || failures=$((failures + 1))
    done
    printf 'curl fallback failures: %s\n' "$failures"
    test "$failures" -eq 0
fi

printf 'stress test finished\n'
