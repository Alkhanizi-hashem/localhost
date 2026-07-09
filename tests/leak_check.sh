#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(dirname "$(dirname "$(readlink -f "$0")")")"
SERVER_BIN="$ROOT_DIR/target/debug/localhost"
CONFIG_PATH="$ROOT_DIR/config/default.conf"
LOG_DIR="/tmp/opencode/localhost-leaks"
SERVER_PID=""

mkdir -p "$LOG_DIR"

cleanup() {
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}

trap cleanup EXIT

cd "$ROOT_DIR"
cargo build >/dev/null

if command -v valgrind >/dev/null 2>&1; then
    valgrind \
        --leak-check=full \
        --show-leak-kinds=all \
        --log-file="$LOG_DIR/valgrind.log" \
        "$SERVER_BIN" "$CONFIG_PATH" &
    SERVER_PID=$!
    sleep 1
    tests/stress.sh
    printf 'valgrind log saved to %s\n' "$LOG_DIR/valgrind.log"
else
    "$SERVER_BIN" "$CONFIG_PATH" >"$LOG_DIR/server.log" 2>&1 &
    SERVER_PID=$!

    for _ in $(seq 1 50); do
        if curl -fsS http://127.0.0.1:8080/ >/dev/null 2>&1; then
            break
        fi
        sleep 0.1
    done

    rss_before="$(ps -o rss= -p "$SERVER_PID" | tr -d ' ')"
    failures=0
    for _ in $(seq 1 500); do
        curl -fsS http://127.0.0.1:8080/ >/dev/null || failures=$((failures + 1))
        curl -fsS http://127.0.0.1:8080/listing/ >/dev/null || failures=$((failures + 1))
    done
    rss_after_first="$(ps -o rss= -p "$SERVER_PID" | tr -d ' ')"

    for _ in $(seq 1 500); do
        curl -fsS http://127.0.0.1:8080/ >/dev/null || failures=$((failures + 1))
        curl -fsS http://127.0.0.1:8080/listing/ >/dev/null || failures=$((failures + 1))
    done
    rss_after_second="$(ps -o rss= -p "$SERVER_PID" | tr -d ' ')"

    for _ in $(seq 1 500); do
        curl -fsS http://127.0.0.1:8080/ >/dev/null || failures=$((failures + 1))
        curl -fsS http://127.0.0.1:8080/listing/ >/dev/null || failures=$((failures + 1))
    done
    rss_after_third="$(ps -o rss= -p "$SERVER_PID" | tr -d ' ')"
    active_connections=0
    for _ in $(seq 1 20); do
        active_connections="$(ss -tanp | awk '$4 ~ /:8080$/ && ($1=="ESTAB" || $1=="CLOSE-WAIT" || $1=="FIN-WAIT-1" || $1=="FIN-WAIT-2" || $1=="SYN-RECV") {count++} END {print count+0}')"
        if [ "$active_connections" -eq 0 ]; then
            break
        fi
        sleep 0.2
    done

    printf 'rss_before=%sKB\n' "$rss_before" | tee "$LOG_DIR/rss-smoke.log"
    printf 'rss_after_first=%sKB\n' "$rss_after_first" | tee -a "$LOG_DIR/rss-smoke.log"
    printf 'rss_after_second=%sKB\n' "$rss_after_second" | tee -a "$LOG_DIR/rss-smoke.log"
    printf 'rss_after_third=%sKB\n' "$rss_after_third" | tee -a "$LOG_DIR/rss-smoke.log"
    printf 'request_failures=%s\n' "$failures" | tee -a "$LOG_DIR/rss-smoke.log"
    printf 'active_connections=%s\n' "$active_connections" | tee -a "$LOG_DIR/rss-smoke.log"

    test "$failures" -eq 0
    test "$active_connections" -eq 0
    printf 'RSS smoke log saved to %s\n' "$LOG_DIR/rss-smoke.log"
fi
