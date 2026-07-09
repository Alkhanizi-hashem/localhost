# Localhost

Rust HTTP/1.1 static server for the localhost subject, without bonus work.

## Run

```sh
cargo run -- config/default.conf
```

Then open `http://127.0.0.1:8080/`.

## Test

```sh
cargo test
```

For an audit-oriented local smoke run:

```sh
tests/audit_smoke.sh
```

Additional audit helpers:

```sh
tests/stress.sh
tests/nginx_compare.sh
tests/browser_smoke.sh
tests/leak_check.sh
```

Manual audit notes are in:

- `tests/browser_check.md`
- `tests/leaks.md`

The integration tests start the compiled server binary and verify:

- static pages and custom errors
- directory listing
- redirects
- uploads and DELETE
- cookies and sessions
- HTTP/1.1 keep-alive on one connection
- multi-port listeners and virtual hosts
- chunked CGI requests
- CGI timeout and failure handling
- mixed valid/invalid server blocks in one config

## Config Syntax

The server reads an nginx-like config with `server` and `route` blocks. Paths are relative to the directory where you start the server.

Supported server directives:

- `host 127.0.0.1;`
- `ports 8080 8081;`
- `server_name localhost test.local;`
- `client_max_body_size 8M;`
- `error_page 404 www/errors/404.html;`

Supported route directives:

- `methods GET POST DELETE;`
- `root www;`
- `redirect 301 /target;`
- `index index.html;`
- `directory_listing on;`
- `upload_store www/uploads;`
- `cgi .py /usr/bin/python3;`

Audit-ready example configs are available in `config/`:

- `default.conf`
- `single-port.conf`
- `multi-port.conf`
- `virtual-hosts.conf`
- `body-limit.conf`
- `duplicate-port.conf`

The implementation uses one process, one thread, non-blocking sockets, and Linux `epoll` for all socket reads and writes. HTTP/1.1 connections stay open by default and close only when requested or when the server terminates the exchange.
