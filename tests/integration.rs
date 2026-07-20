use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct TestServer {
    child: Child,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

#[test]
fn end_to_end_http_features_work() {
    let workspace = TestWorkspace::new("features");
    workspace.create_site();
    let port = free_port();

    workspace.write_config(
        "default.conf",
        &format!(
            "request_timeout 2;\n\
             cgi_timeout 2;\n\
             server {{\n\
                 host 127.0.0.1;\n\
                 ports {port};\n\
                 server_name localhost;\n\
                 client_max_body_size 32;\n\
                 error_page 400 www/errors/400.html;\n\
                 error_page 403 www/errors/403.html;\n\
                 error_page 404 www/errors/404.html;\n\
                 error_page 405 www/errors/405.html;\n\
                 error_page 413 www/errors/413.html;\n\
                 error_page 500 www/errors/500.html;\n\
                 route / {{\n\
                     root www;\n\
                     methods GET POST DELETE;\n\
                     index index.html;\n\
                     directory_listing off;\n\
                 }}\n\
                 route /uploads {{\n\
                     root www/uploads;\n\
                     methods GET POST DELETE;\n\
                     index index.html;\n\
                     directory_listing on;\n\
                     upload_store www/uploads;\n\
                 }}\n\
                 route /listing {{\n\
                     root www/listing;\n\
                     methods GET;\n\
                     directory_listing on;\n\
                 }}\n\
                 route /cgi {{\n\
                     root www/cgi;\n\
                     methods GET POST;\n\
                     cgi .py /usr/bin/python3;\n\
                 }}\n\
                 route /old {{\n\
                     methods GET;\n\
                     redirect 301 /;\n\
                 }}\n\
             }}\n"
        ),
    );

    let _server = workspace.start("default.conf", &[port]);

    let response = http_request(
        port,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 200);
    assert_eq!(
        response.headers.get("content-type"),
        Some(&"text/html; charset=utf-8".to_string())
    );
    assert_eq!(
        response.headers.get("connection"),
        Some(&"close".to_string())
    );
    assert_eq!(
        response.headers.get("content-length"),
        Some(&response.body.len().to_string())
    );
    assert!(String::from_utf8_lossy(&response.body).contains("Localhost Test Home"));

    let response = http_request(
        port,
        b"GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 404);
    assert!(String::from_utf8_lossy(&response.body).contains("Custom 404"));

    let response = http_request(
        port,
        b"GET /listing/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 200);
    let listing_body = String::from_utf8_lossy(&response.body);
    assert!(listing_body.contains("alpha.txt"));
    assert!(listing_body.contains("beta.txt"));

    let response = http_request(
        port,
        b"DELETE /listing/alpha.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 405);
    assert_eq!(response.headers.get("allow"), Some(&"GET".to_string()));

    let response = http_request(
        port,
        b"GET /old HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 301);
    assert_eq!(response.headers.get("location"), Some(&"/".to_string()));

    let upload_body = [0u8, 1, 2, 3, 4, 200, 201, 202];
    let upload_request = format!(
        "POST /uploads HTTP/1.1\r\nHost: localhost\r\nX-Filename: sample.bin\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        upload_body.len()
    );
    let mut bytes = upload_request.into_bytes();
    bytes.extend_from_slice(&upload_body);
    let response = http_request(port, &bytes);
    assert_eq!(response.status, 201);

    let response = http_request(
        port,
        b"GET /uploads/sample.bin HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 200);
    assert_eq!(response.body, upload_body);

    let response = http_request(
        port,
        b"DELETE /uploads/sample.bin HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 204);

    let response = http_request(
        port,
        b"GET /session HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 200);
    assert_eq!(
        response.headers.get("x-session-visits"),
        Some(&"1".to_string())
    );
    let cookie = response
        .headers
        .get("set-cookie")
        .and_then(|value| value.split(';').next())
        .expect("missing session cookie")
        .to_string();

    let response = http_request(
        port,
        format!(
            "GET /session HTTP/1.1\r\nHost: localhost\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n"
        )
        .as_bytes(),
    );
    assert_eq!(response.status, 200);
    assert_eq!(
        response.headers.get("x-session-visits"),
        Some(&"2".to_string())
    );

    let response = http_request(
        port,
        b"POST /uploads HTTP/1.1\r\nHost: localhost\r\nContent-Length: 64\r\nConnection: close\r\n\r\n0123456789012345678901234567890123456789012345678901234567890123",
    );
    assert_eq!(response.status, 413);
}

#[test]
fn supports_virtual_hosts_and_multiple_ports() {
    let workspace = TestWorkspace::new("vhosts");
    workspace.create_site();
    workspace.write_file("www/a/index.html", "A host\n");
    workspace.write_file("www/b/index.html", "B host\n");
    workspace.write_file("www/second/index.html", "Second port\n");
    let shared_port = free_port();
    let second_port = free_port();

    workspace.write_config(
        "vhosts.conf",
        &format!(
            "server {{\n\
                 host 127.0.0.1;\n\
                 ports {shared_port};\n\
                 server_name a.test;\n\
                 route / {{ root www/a; methods GET; index index.html; directory_listing off; }}\n\
             }}\n\
             server {{\n\
                 host 127.0.0.1;\n\
                 ports {shared_port};\n\
                 server_name b.test;\n\
                 route / {{ root www/b; methods GET; index index.html; directory_listing off; }}\n\
             }}\n\
             server {{\n\
                 host 127.0.0.1;\n\
                 ports {second_port};\n\
                 route / {{ root www/second; methods GET; index index.html; directory_listing off; }}\n\
             }}\n"
        ),
    );

    let _server = workspace.start("vhosts.conf", &[shared_port, second_port]);

    let response = http_request(
        shared_port,
        b"GET / HTTP/1.1\r\nHost: a.test\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 200);
    assert!(String::from_utf8_lossy(&response.body).contains("A host"));

    let response = http_request(
        shared_port,
        b"GET / HTTP/1.1\r\nHost: b.test\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 200);
    assert!(String::from_utf8_lossy(&response.body).contains("B host"));

    let response = http_request(
        shared_port,
        b"GET / HTTP/1.1\r\nHost: unknown.test\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 200);
    assert!(String::from_utf8_lossy(&response.body).contains("A host"));

    let response = http_request(
        second_port,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 200);
    assert!(String::from_utf8_lossy(&response.body).contains("Second port"));
}

#[test]
fn handles_chunked_cgi_and_discards_invalid_server_blocks() {
    let workspace = TestWorkspace::new("cgi-and-invalid");
    workspace.create_site();
    let valid_port = free_port();

    workspace.write_config(
        "mixed.conf",
        &format!(
            "request_timeout 2;\n\
             cgi_timeout 2;\n\
             server {{\n\
                 host 127.0.0.1;\n\
                 ports {valid_port} {valid_port};\n\
                 route / {{ root www; methods GET; index index.html; directory_listing off; }}\n\
             }}\n\
             server {{\n\
                 host 127.0.0.1;\n\
                 ports {valid_port};\n\
                 client_max_body_size 128;\n\
                 route / {{ root www; methods GET; index index.html; directory_listing off; }}\n\
                 route /cgi {{ root www/cgi; methods GET POST; cgi .py /usr/bin/python3; }}\n\
             }}\n"
        ),
    );

    let _server = workspace.start("mixed.conf", &[valid_port]);

    let response = http_request(
        valid_port,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 200);

    let response = http_request(
        valid_port,
        b"POST /cgi/echo.py?source=test HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n",
    );
    assert_eq!(response.status, 200);
    let body = String::from_utf8_lossy(&response.body);
    assert!(body.contains("body bytes</dt><dd>11</dd>"));
    assert!(body.contains("hello world"));
    assert!(body.contains("source=test"));
}

#[test]
fn handles_unchunked_cgi_and_bad_requests_without_crashing() {
    let workspace = TestWorkspace::new("cgi-and-bad-request");
    workspace.create_site();
    let port = free_port();

    workspace.write_config(
        "server.conf",
        &format!(
            "request_timeout 2;\n\
             cgi_timeout 2;\n\
             server {{\n\
                 host 127.0.0.1;\n\
                 ports {port};\n\
                 error_page 400 www/errors/400.html;\n\
                 route / {{ root www; methods GET; index index.html; directory_listing off; }}\n\
                 route /cgi {{ root www/cgi; methods GET POST; cgi .py /usr/bin/python3; }}\n\
             }}\n"
        ),
    );

    let _server = workspace.start("server.conf", &[port]);

    let response = http_request(
        port,
        b"POST /cgi/echo.py?mode=plain HTTP/1.1\r\nHost: localhost\r\nContent-Length: 11\r\nConnection: close\r\n\r\nhello world",
    );
    assert_eq!(response.status, 200);
    let body = String::from_utf8_lossy(&response.body);
    assert!(body.contains("body bytes</dt><dd>11</dd>"));
    assert!(body.contains("hello world"));
    assert!(body.contains("mode=plain"));

    let response = http_request(port, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n");
    assert_eq!(response.status, 400);
    assert!(String::from_utf8_lossy(&response.body).contains("Custom 400"));

    let response = http_request(
        port,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 200);
    assert!(String::from_utf8_lossy(&response.body).contains("Localhost Test Home"));
}

#[test]
fn times_out_slow_requests_without_hanging() {
    let workspace = TestWorkspace::new("timeout");
    workspace.create_site();
    let port = free_port();

    workspace.write_config(
        "timeout.conf",
        &format!(
            "request_timeout 1;\n\
             server {{\n\
                 host 127.0.0.1;\n\
                 ports {port};\n\
                 route / {{ root www; methods GET; index index.html; directory_listing off; }}\n\
             }}\n"
        ),
    );

    let _server = workspace.start("timeout.conf", &[port]);

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect failed");
    stream
        .set_read_timeout(Some(Duration::from_secs(4)))
        .expect("set read timeout failed");
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
        .expect("write partial request failed");

    thread::sleep(Duration::from_millis(1400));

    let mut response = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => response.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(error) => panic!("read failed: {error}"),
        }
    }

    let response = parse_response(&response);
    assert_eq!(response.status, 408);

    let response = http_request(
        port,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 200);
}

#[test]
fn supports_http11_keep_alive_on_same_connection() {
    let workspace = TestWorkspace::new("keepalive");
    workspace.create_site();
    let port = free_port();

    workspace.write_config(
        "keepalive.conf",
        &format!(
            "request_timeout 2;\n\
             server {{\n\
                 host 127.0.0.1;\n\
                 ports {port};\n\
                 route / {{ root www; methods GET; index index.html; directory_listing off; }}\n\
                 route /listing {{ root www/listing; methods GET; directory_listing on; }}\n\
             }}\n"
        ),
    );

    let _server = workspace.start("keepalive.conf", &[port]);

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect failed");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("set read timeout failed");
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .expect("write failed");

    let first = read_response(&mut stream);
    assert_eq!(first.status, 200);
    assert_eq!(
        first.headers.get("connection"),
        Some(&"keep-alive".to_string())
    );
    assert!(String::from_utf8_lossy(&first.body).contains("Localhost Test Home"));

    stream
        .write_all(b"GET /listing/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("write second request failed");

    let second = read_response(&mut stream);
    assert_eq!(second.status, 200);
    assert_eq!(second.headers.get("connection"), Some(&"close".to_string()));
    assert!(String::from_utf8_lossy(&second.body).contains("alpha.txt"));
}

#[test]
fn supports_pipelined_requests_without_overwriting_responses() {
    let workspace = TestWorkspace::new("pipelining");
    workspace.create_site();
    let port = free_port();

    workspace.write_config(
        "pipelining.conf",
        &format!(
            "request_timeout 2;\n\
             server {{\n\
                 host 127.0.0.1;\n\
                 ports {port};\n\
                 route / {{ root www; methods GET; index index.html; directory_listing off; }}\n\
                 route /listing {{ root www/listing; methods GET; directory_listing on; }}\n\
             }}\n"
        ),
    );

    let _server = workspace.start("pipelining.conf", &[port]);
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect failed");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("set read timeout failed");
    stream
        .write_all(
            b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\nGET /listing/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .expect("write pipelined requests failed");

    let mut responses = Vec::new();
    stream
        .read_to_end(&mut responses)
        .expect("read pipelined responses failed");
    assert_eq!(
        responses
            .windows(b"HTTP/1.1 200 OK".len())
            .filter(|window| *window == b"HTTP/1.1 200 OK")
            .count(),
        2
    );
    let body = String::from_utf8_lossy(&responses);
    assert!(body.contains("Localhost Test Home"));
    assert!(body.contains("alpha.txt"));
}

#[test]
fn returns_expected_statuses_for_protocol_and_cgi_edge_cases() {
    let workspace = TestWorkspace::new("statuses");
    workspace.create_site();
    workspace.write_file(
        "www/cgi/slow.py",
        "#!/usr/bin/env python3\nimport time\ntime.sleep(2)\nprint('Content-Type: text/plain')\nprint()\nprint('slow')\n",
    );
    workspace.write_file(
        "www/cgi/fail.py",
        "#!/usr/bin/env python3\nimport sys\nsys.exit(1)\n",
    );
    let port = free_port();

    workspace.write_config(
        "statuses.conf",
        &format!(
            "cgi_timeout 1;\n\
             server {{\n\
                 host 127.0.0.1;\n\
                 ports {port};\n\
                 error_page 400 www/errors/400.html;\n\
                 error_page 403 www/errors/403.html;\n\
                 error_page 404 www/errors/404.html;\n\
                 error_page 405 www/errors/405.html;\n\
                 error_page 413 www/errors/413.html;\n\
                 error_page 500 www/errors/500.html;\n\
                 route / {{ root www; methods GET; index index.html; directory_listing off; }}\n\
                 route /listing {{ root www/listing; methods GET; directory_listing off; }}\n\
                 route /cgi {{ root www/cgi; methods GET POST; cgi .py /usr/bin/python3; }}\n\
             }}\n"
        ),
    );

    let _server = workspace.start("statuses.conf", &[port]);

    let response = http_request(
        port,
        b"PUT / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 405);
    assert_eq!(response.headers.get("allow"), Some(&"GET".to_string()));

    let response = http_request(
        port,
        b"GET /listing/../index.html HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 403);
    assert!(String::from_utf8_lossy(&response.body).contains("Custom 403"));

    let response = http_request(
        port,
        b"POST /cgi/echo.py HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nz\r\nhello\r\n0\r\n\r\n",
    );
    assert_eq!(response.status, 400);
    assert!(String::from_utf8_lossy(&response.body).contains("Custom 400"));

    let response = http_request(
        port,
        b"GET /cgi/slow.py HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 504);

    let response = http_request(
        port,
        b"GET /cgi/fail.py HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 500);
    assert!(String::from_utf8_lossy(&response.body).contains("Custom 500"));
}

#[test]
fn slow_cgi_does_not_block_other_clients() {
    let workspace = TestWorkspace::new("cgi-concurrency");
    workspace.create_site();
    workspace.write_file(
        "www/cgi/slow.py",
        "#!/usr/bin/env python3\nimport time\ntime.sleep(1.2)\nprint('Content-Type: text/plain')\nprint()\nprint('slow')\n",
    );
    let port = free_port();
    workspace.write_config(
        "concurrency.conf",
        &format!(
            "request_timeout 3;\n\
             cgi_timeout 3;\n\
             server {{\n\
                 host 127.0.0.1;\n\
                 ports {port};\n\
                 route / {{ root www; methods GET; index index.html; directory_listing off; }}\n\
                 route /cgi {{ root www/cgi; methods GET POST; cgi .py /usr/bin/python3; }}\n\
             }}\n"
        ),
    );

    let _server = workspace.start("concurrency.conf", &[port]);
    let slow_request = thread::spawn(move || {
        http_request(
            port,
            b"GET /cgi/slow.py HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
    });
    thread::sleep(Duration::from_millis(150));

    let started = Instant::now();
    let static_response = http_request(
        port,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    let static_elapsed = started.elapsed();

    assert_eq!(static_response.status, 200);
    assert!(
        static_elapsed < Duration::from_millis(700),
        "static request was blocked by CGI for {static_elapsed:?}"
    );
    let slow_response = slow_request.join().expect("slow CGI request panicked");
    assert_eq!(slow_response.status, 200);
    assert!(String::from_utf8_lossy(&slow_response.body).contains("slow"));
}

#[test]
fn survives_repeated_malformed_and_slow_clients() {
    let workspace = TestWorkspace::new("resilience");
    workspace.create_site();
    let port = free_port();

    workspace.write_config(
        "resilience.conf",
        &format!(
            "request_timeout 1;\n\
             cgi_timeout 1;\n\
             server {{\n\
                 host 127.0.0.1;\n\
                 ports {port};\n\
                 error_page 400 www/errors/400.html;\n\
                 route / {{ root www; methods GET; index index.html; directory_listing off; }}\n\
                 route /cgi {{ root www/cgi; methods GET POST; cgi .py /usr/bin/python3; }}\n\
             }}\n"
        ),
    );

    let _server = workspace.start("resilience.conf", &[port]);

    for request in [
        b"GET / HTTP/1.1\r\n\r\n".as_slice(),
        b"BAD / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        b"POST /cgi/echo.py HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n1\r\na",
        b"GET /%ZZ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    ] {
        let response = http_request(port, request);
        assert!(matches!(response.status, 400 | 405 | 408));
    }

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect failed");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("set read timeout failed");
    stream
        .write_all(b"POST /cgi/echo.py HTTP/1.1\r\nHost: localhost\r\nContent-Length: 20\r\n")
        .expect("write partial request failed");
    thread::sleep(Duration::from_millis(1300));
    let timeout_response = read_response(&mut stream);
    assert_eq!(timeout_response.status, 408);

    let response = http_request(
        port,
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.status, 200);
    assert!(String::from_utf8_lossy(&response.body).contains("Localhost Test Home"));
}

fn http_request(port: u16, request: &[u8]) -> HttpResponse {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect failed");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("set read timeout failed");
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .expect("set write timeout failed");
    stream.write_all(request).expect("write failed");

    let mut response = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => response.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(error) => panic!("read failed: {error}"),
        }
    }
    parse_response(&response)
}

fn parse_response(bytes: &[u8]) -> HttpResponse {
    let marker = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4))
        .or_else(|| {
            bytes
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|index| (index, 2))
        })
        .expect("response missing header separator");
    let header_text = String::from_utf8_lossy(&bytes[..marker.0]);
    let mut lines = header_text.lines();
    let status_line = lines.next().expect("missing status line");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .expect("missing status code")
        .parse::<u16>()
        .expect("invalid status code");
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    HttpResponse {
        status,
        headers,
        body: bytes[marker.0 + marker.1..].to_vec(),
    }
}

fn read_response(stream: &mut TcpStream) -> HttpResponse {
    let mut response = Vec::new();
    let mut buffer = [0u8; 4096];
    let mut expected_length = None;
    let mut header_size = None;

    loop {
        let read = stream.read(&mut buffer).expect("read failed");
        assert!(read > 0, "connection closed before full response");
        response.extend_from_slice(&buffer[..read]);

        if header_size.is_none() {
            if let Some((header_end, separator_len)) = response
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| (index, 4))
                .or_else(|| {
                    response
                        .windows(2)
                        .position(|window| window == b"\n\n")
                        .map(|index| (index, 2))
                })
            {
                let header_text = String::from_utf8_lossy(&response[..header_end]);
                expected_length = header_text.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.trim().eq_ignore_ascii_case("content-length") {
                        value.trim().parse::<usize>().ok()
                    } else {
                        None
                    }
                });
                header_size = Some(header_end + separator_len);
            }
        }

        if let (Some(header_size), Some(expected_length)) = (header_size, expected_length) {
            if response.len() >= header_size + expected_length {
                return parse_response(&response[..header_size + expected_length]);
            }
        }
    }
}

struct TestWorkspace {
    root: PathBuf,
}

impl TestWorkspace {
    fn new(label: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/tmp/opencode/localhost-test-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create workspace failed");
        Self { root }
    }

    fn create_site(&self) {
        self.write_file(
            "www/index.html",
            "<!doctype html><html><body><h1>Localhost Test Home</h1></body></html>\n",
        );
        self.write_file(
            "www/uploads/index.html",
            "<!doctype html><html><body><h1>Uploads</h1></body></html>\n",
        );
        self.write_file("www/listing/alpha.txt", "alpha\n");
        self.write_file("www/listing/beta.txt", "beta\n");
        self.write_file(
            "www/cgi/echo.py",
            "#!/usr/bin/env python3\nimport os\nimport sys\nbody = sys.stdin.buffer.read()\nprint('Content-Type: text/html; charset=utf-8')\nprint()\nprint('<!doctype html><html><body>')\nprint('<h1>CGI echo</h1>')\nprint('<dl>')\nprint(f'<dt>query</dt><dd>{os.environ.get(\"QUERY_STRING\", \"\")}</dd>')\nprint(f'<dt>path_info</dt><dd>{os.environ.get(\"PATH_INFO\", \"\")}</dd>')\nprint(f'<dt>body bytes</dt><dd>{len(body)}</dd>')\nprint('</dl>')\nif body:\n    print('<pre>')\n    print(body.decode('utf-8', 'replace'))\n    print('</pre>')\nprint('</body></html>')\n",
        );
        self.write_file("www/errors/400.html", "Custom 400\n");
        self.write_file("www/errors/403.html", "Custom 403\n");
        self.write_file("www/errors/404.html", "Custom 404\n");
        self.write_file("www/errors/405.html", "Custom 405\n");
        self.write_file("www/errors/413.html", "Custom 413\n");
        self.write_file("www/errors/500.html", "Custom 500\n");
    }

    fn write_config(&self, relative_path: &str, contents: &str) {
        self.write_file(relative_path, contents);
    }

    fn write_file(&self, relative_path: &str, contents: &str) {
        let path = self.root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir failed");
        }
        fs::write(path, contents).expect("write file failed");
    }

    fn start(&self, config_name: &str, ports: &[u16]) -> TestServer {
        let mut child = Command::new(env!("CARGO_BIN_EXE_localhost"))
            .arg(config_name)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("server spawn failed");

        for _ in 0..60 {
            if ports
                .iter()
                .all(|port| TcpStream::connect(("127.0.0.1", *port)).is_ok())
            {
                return TestServer { child };
            }
            if let Some(status) = child.try_wait().expect("try_wait failed") {
                panic!("server exited early with status {status}");
            }
            thread::sleep(Duration::from_millis(50));
        }

        panic!("server did not start listening in time");
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = remove_dir_all_if_exists(&self.root);
    }
}

fn remove_dir_all_if_exists(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)
    } else {
        Ok(())
    }
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("bind free port probe failed")
        .local_addr()
        .expect("local_addr failed")
        .port()
}
