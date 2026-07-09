#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(dirname "$(dirname "$(readlink -f "$0")")")"
SERVER_BIN="$ROOT_DIR/target/debug/localhost"
CONFIG_PATH="$ROOT_DIR/config/default.conf"
SERVER_PID=""
NGINX_PID=""
TMP_DIR="/tmp/opencode/localhost-nginx-compare"

cleanup() {
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    if [ -n "$NGINX_PID" ] && kill -0 "$NGINX_PID" 2>/dev/null; then
        kill "$NGINX_PID" 2>/dev/null || true
        wait "$NGINX_PID" 2>/dev/null || true
    fi
}

trap cleanup EXIT

if ! command -v nginx >/dev/null 2>&1; then
    printf 'nginx is not installed\n' >&2
    exit 1
fi

mkdir -p "$TMP_DIR"

cat >"$TMP_DIR/nginx.conf" <<EOF
events {}
http {
    server {
        listen 8088;
        server_name localhost;
        root $ROOT_DIR/www;
        index index.html;

        error_page 400 /errors/400.html;
        error_page 403 /errors/403.html;
        error_page 404 /errors/404.html;
        error_page 405 /errors/405.html;
        error_page 413 /errors/413.html;
        error_page 500 /errors/500.html;

        location /listing/ {
            alias $ROOT_DIR/www/listing/;
            autoindex on;
        }

        location /old {
            return 301 /;
        }

        location / {
            try_files \$uri \$uri/ =404;
        }
    }
}
EOF

cd "$ROOT_DIR"
cargo build >/dev/null

"$SERVER_BIN" "$CONFIG_PATH" >"$TMP_DIR/server.log" 2>&1 &
SERVER_PID=$!

nginx -c "$TMP_DIR/nginx.conf" -g 'daemon off;' >"$TMP_DIR/nginx.log" 2>&1 &
NGINX_PID=$!

for _ in $(seq 1 50); do
    if curl -fsS http://127.0.0.1:8080/ >/dev/null 2>&1 && curl -fsS http://127.0.0.1:8088/ >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done

compare() {
    local path="$1"
    local method="${2:-GET}"
    local body="${3:-}"
    local left="$TMP_DIR/left.txt"
    local right="$TMP_DIR/right.txt"

    if [ -n "$body" ]; then
        curl -sS -X "$method" --data "$body" -D - "http://127.0.0.1:8080$path" >"$left"
        curl -sS -X "$method" --data "$body" -D - "http://127.0.0.1:8088$path" >"$right"
    else
        curl -sS -X "$method" -D - "http://127.0.0.1:8080$path" >"$left"
        curl -sS -X "$method" -D - "http://127.0.0.1:8088$path" >"$right"
    fi

    printf '=== %s %s ===\n' "$method" "$path"
    diff -u "$left" "$right" || true
}

compare /
compare /missing
compare /listing/
compare /old

printf 'nginx comparison finished\n'
