# Localhost HTTP Server

## Overview

`localhost` is a dependency-free Rust HTTP/1.0 and HTTP/1.1 server for Linux.
It uses one `epoll` event loop for listeners, clients, and CGI pipes, allowing
static requests to continue while CGI child processes are running.

## Implemented Features

- Non-blocking TCP listeners and clients managed by a single Linux `epoll`
  instance.
- `GET`, `POST`, and `DELETE` route handling.
- HTTP/1.0 and HTTP/1.1 parsing, including required HTTP/1.1 `Host` headers.
- Request bodies using `Content-Length` or chunked transfer encoding.
- HTTP/1.1 keep-alive, HTTP/1.0 keep-alive when requested, and pipelined
  requests on one connection.
- Static file serving with extension-based content types.
- Index files, generated directory listings, redirects, and custom error pages.
- Per-server request body limits plus global request and CGI timeouts.
- Raw and `multipart/form-data` uploads with unique, sanitized filenames.
- File deletion for routes that allow `DELETE`.
- Extension-to-interpreter CGI mappings, CGI environment variables,
  non-blocking stdin/stdout, and execution timeouts.
- Multiple ports and name-based virtual hosts sharing a listener.
- In-memory sessions using the `LOCALHOST_SESSION` cookie. `/session` displays
  the current session ID and visit count; sessions expire after one hour of
  inactivity.

## Architecture

- `src/config.rs` tokenizes, parses, defaults, and validates the nginx-like
  configuration format.
- `src/http.rs` parses requests, decodes chunked bodies, and serializes
  responses.
- `src/router.rs` performs longest-prefix route selection and handles files,
  listings, redirects, uploads, deletion, and CGI dispatch.
- `src/cgi.rs` starts interpreters, streams request/response data, and parses CGI
  response headers.
- `src/server.rs` owns listeners, connections, sessions, timeouts, virtual-host
  selection, and the event loop.
- `src/epoll.rs` and `src/ffi.rs` provide the Linux `epoll` and non-blocking file
  descriptor wrappers.
- `src/util.rs` contains path, MIME, escaping, hostname, and encoding helpers.

## Prerequisites

- Linux. The server calls `epoll_create1`, `epoll_ctl`, `epoll_wait`, and Linux
  `fcntl` constants directly; it does not run on macOS or Windows.
- A Rust toolchain with Cargo and Rust 2021 edition support.
- Python 3 at `/usr/bin/python3` to use the CGI route in
  `config/default.conf`.
- `curl` and Bash for the supplied smoke scripts. `siege`, `nginx`, and
  `valgrind` are optional and used only by specific audit helpers.

## Build

From the repository root on Linux:

```sh
cargo build --release
```

The executable is written to `target/release/localhost`.

## Run

The executable accepts one optional configuration path. If omitted, it uses
`config/default.conf`.

```sh
cargo run --release -- config/default.conf
```

Equivalent direct invocation:

```sh
./target/release/localhost config/default.conf
```

The default configuration listens at `127.0.0.1:8080`. Example requests:

```sh
curl -i http://127.0.0.1:8080/
curl -i http://127.0.0.1:8080/listing/
curl -i 'http://127.0.0.1:8080/cgi/echo.py?source=readme'
curl -i http://127.0.0.1:8080/session
```

Upload and remove a file through the configured `/uploads` route:

```sh
curl -i -X POST -H 'X-Filename: example.txt' \
  --data-binary 'example payload' http://127.0.0.1:8080/uploads
curl -i -X DELETE http://127.0.0.1:8080/uploads/example.txt
```

## Configuration

Configuration and content paths are resolved relative to the directory from
which the server is started. A minimal complete configuration is:

```nginx
request_timeout 30;
cgi_timeout 5;

server {
    host 127.0.0.1;
    ports 8080;
    server_name localhost;
    client_max_body_size 8M;
    error_page 404 www/errors/404.html;

    route / {
        root www;
        methods GET POST DELETE;
        index index.html;
        directory_listing off;
    }

    route /cgi {
        root www/cgi;
        methods GET POST;
        cgi .py /usr/bin/python3;
    }
}
```

Supported top-level directives are `request_timeout` and `cgi_timeout`, both in
seconds. Server blocks support `host`, `port`/`ports`/`listen`, `server_name`,
`client_max_body_size`, `error_page`, and `route`. Body sizes accept bytes or
`k`, `m`, and `g` suffixes.

Route blocks support `methods`, `root`, `redirect`, `index`/`default_file`,
`directory_listing`/`autoindex`, `upload_store`, and `cgi`. A redirect without
an explicit status defaults to `302`.

Additional configurations in `config/` demonstrate one or multiple ports,
virtual hosts, body limits, and invalid duplicate-port handling.

## Tests

Run unit and integration tests on Linux:

```sh
cargo test
```

The integration suite starts isolated server processes on available ports and
covers static content, errors, listings, redirects, uploads, deletion, sessions,
virtual hosts, chunked CGI, keep-alive, pipelining, timeouts, and concurrent CGI
traffic.

Optional audit helpers are:

```sh
./tests/audit_smoke.sh
./tests/browser_smoke.sh
./tests/stress.sh
./tests/nginx_compare.sh
./tests/leak_check.sh
```

`nginx_compare.sh` requires nginx. `leak_check.sh` uses Valgrind when available
and otherwise performs an RSS/connection smoke check.

## Project Structure

```text
.
|-- Cargo.toml
|-- config/             # sample server configurations
|-- src/                # parser, router, event loop, HTTP, and CGI code
|-- tests/              # Rust integration tests and audit scripts
|-- www/                # sample site, CGI script, uploads, and error pages
`-- README.md
```
