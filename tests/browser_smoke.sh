#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(dirname "$(dirname "$(readlink -f "$0")")")"
SERVER_BIN="$ROOT_DIR/target/debug/localhost"
CONFIG_PATH="$ROOT_DIR/config/default.conf"
SERVER_PID=""
SERVER_URL="http://127.0.0.1:8080"

cleanup() {
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}

trap cleanup EXIT

cd "$ROOT_DIR"
cargo build >/dev/null

"$SERVER_BIN" "$CONFIG_PATH" >/tmp/opencode/localhost-browser-smoke.log 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 50); do
    if curl -fsS "$SERVER_URL/" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done

curl -fsS "$SERVER_URL/" | grep -q 'Localhost'
curl -fsS -o /dev/null -D /tmp/opencode/localhost-browser-headers.txt "$SERVER_URL/session"
grep -qi '^set-cookie:' /tmp/opencode/localhost-browser-headers.txt
curl -fsS -o /dev/null -w '%{http_code}\n' "$SERVER_URL/old" | grep -qx 301
curl -fsS "$SERVER_URL/listing/" | grep -q 'alpha.txt'
curl -fsS "$SERVER_URL/cgi/echo.py?source=browser" | grep -q 'CGI echo'

printf 'browser smoke finished\n'
