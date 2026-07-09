#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(dirname "$(dirname "$(readlink -f "$0")")")"
SERVER_BIN="$ROOT_DIR/target/debug/localhost"
CONFIG_PATH="$ROOT_DIR/config/default.conf"
SERVER_URL="http://127.0.0.1:8080"
SERVER_PID=""

cleanup() {
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}

trap cleanup EXIT

cd "$ROOT_DIR"
cargo test

"$SERVER_BIN" "$CONFIG_PATH" >/tmp/opencode/localhost-audit-smoke.log 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 50); do
    if curl -fsS "$SERVER_URL/" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done

curl -fsS "$SERVER_URL/" >/dev/null
curl -fsS "$SERVER_URL/listing/" >/dev/null
curl -fsS "$SERVER_URL/cgi/echo.py?source=smoke" >/dev/null
curl -fsS -X POST -H "Content-Type: text/plain" --data "hello world" "$SERVER_URL/cgi/echo.py" >/dev/null
curl -sS -o /dev/null -w "%{http_code}\n" "$SERVER_URL/missing" | grep -qx 404
curl -sS -o /dev/null -w "%{http_code}\n" "$SERVER_URL/old" | grep -qx 301

if command -v siege >/dev/null 2>&1; then
    siege -b 127.0.0.1:8080
else
    echo "siege not installed, using curl fallback stress loop"
    failures=0
    for _ in $(seq 1 200); do
        curl -fsS "$SERVER_URL/" >/dev/null || failures=$((failures + 1))
    done
    echo "curl fallback failures: $failures"
    test "$failures" -eq 0
fi

echo "audit smoke passed"
