use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use crate::epoll::Epoll;
use crate::ffi::{set_nonblocking, EpollEvent, EPOLLERR, EPOLLHUP, EPOLLIN, EPOLLOUT, EPOLLRDHUP};
use crate::http::{Request, Response};

const CGI_STDOUT_EVENTS: u32 = EPOLLIN | EPOLLERR | EPOLLHUP | EPOLLRDHUP;
const CGI_STDIN_EVENTS: u32 = EPOLLOUT | EPOLLERR | EPOLLHUP | EPOLLRDHUP;
const CGI_BUFFER_SIZE: usize = 16 * 1024;

pub enum CgiError {
    Io(io::Error),
    Timeout,
    InvalidOutput,
}

impl From<io::Error> for CgiError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn run(
    request: &Request,
    script_path: &Path,
    interpreter: &Path,
    timeout: Duration,
) -> Result<Response, CgiError> {
    let script_full_path =
        std::fs::canonicalize(script_path).unwrap_or_else(|_| script_path.to_path_buf());

    let mut command = Command::new(interpreter);
    command
        .arg(&script_full_path)
        .current_dir(script_full_path.parent().unwrap_or_else(|| Path::new(".")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GATEWAY_INTERFACE", "CGI/1.1")
        .env("REQUEST_METHOD", request.method.to_string())
        .env("SCRIPT_FILENAME", &script_full_path)
        .env("SCRIPT_NAME", request.path.as_str())
        .env("PATH_INFO", &script_full_path)
        .env("REQUEST_URI", request.target.as_str())
        .env("QUERY_STRING", request.query.as_str())
        .env("SERVER_PROTOCOL", request.version.as_str())
        .env("CONTENT_LENGTH", request.body.len().to_string());

    if let Some(content_type) = request.header("content-type") {
        command.env("CONTENT_TYPE", content_type);
    }
    for (name, value) in &request.headers {
        let env_name = format!(
            "HTTP_{}",
            name.chars()
                .map(|ch| if ch == '-' { '_' } else { ch.to_ascii_uppercase() })
                .collect::<String>()
        );
        command.env(env_name, value);
    }

    let mut child = command.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "missing CGI stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "missing CGI stdout"))?;

    set_nonblocking(stdin.as_raw_fd())?;
    set_nonblocking(stdout.as_raw_fd())?;

    drive_cgi(child, stdin, stdout, &request.body, timeout)
}

fn drive_cgi(
    mut child: Child,
    stdin: ChildStdin,
    mut stdout: ChildStdout,
    body: &[u8],
    timeout: Duration,
) -> Result<Response, CgiError> {
    let epoll = Epoll::new()?;
    let stdin_fd = stdin.as_raw_fd();
    let stdout_fd = stdout.as_raw_fd();
    epoll.add(stdin_fd, CGI_STDIN_EVENTS)?;
    epoll.add(stdout_fd, CGI_STDOUT_EVENTS)?;

    let started = Instant::now();
    let mut events = vec![EpollEvent::empty(); 8];
    let mut output = Vec::new();
    let mut written = 0usize;
    let mut stdin_open = true;
    let mut stdin = Some(stdin);
    let mut stdout_open = true;
    let mut child_exited = false;

    while stdout_open || !child_exited {
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CgiError::Timeout);
        }

        let remaining = timeout
            .checked_sub(started.elapsed())
            .unwrap_or_else(|| Duration::from_millis(0));
        let timeout_ms = remaining.as_millis().min(i32::MAX as u128) as i32;
        let ready = epoll.wait(&mut events, timeout_ms)?;

        for event in events.iter().take(ready) {
            let fd = event.data as i32;

            if fd == stdin_fd && stdin_open {
                if event.events & (EPOLLERR | EPOLLHUP | EPOLLRDHUP) != 0 {
                    stdin_open = false;
                } else if event.events & EPOLLOUT != 0 {
                    while written < body.len() {
                        let Some(stdin) = stdin.as_mut() else {
                            stdin_open = false;
                            break;
                        };
                        match stdin.write(&body[written..]) {
                            Ok(0) => break,
                            Ok(bytes) => written += bytes,
                            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                            Err(error) => return Err(CgiError::Io(error)),
                        }
                    }

                    if written >= body.len() {
                        epoll.delete(stdin_fd);
                        stdin.take();
                        stdin_open = false;
                    }
                }
            }

            if fd == stdout_fd && stdout_open {
                let mut buffer = [0u8; CGI_BUFFER_SIZE];
                loop {
                    match stdout.read(&mut buffer) {
                        Ok(0) => {
                            epoll.delete(stdout_fd);
                            stdout_open = false;
                            break;
                        }
                        Ok(bytes) => output.extend_from_slice(&buffer[..bytes]),
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) => return Err(CgiError::Io(error)),
                    }
                }
            }
        }

        child_exited = child.try_wait()?.is_some();

        if stdin_open && written >= body.len() {
            epoll.delete(stdin_fd);
            stdin.take();
            stdin_open = false;
        }

        if child_exited && !stdout_open {
            break;
        }
    }

    let exit_status = child.wait()?;
    if !exit_status.success() {
        return Err(CgiError::InvalidOutput);
    }

    Ok(parse_output(&output))
}

fn parse_output(output: &[u8]) -> Response {
    let (header_bytes, body) = match find_header_end(output) {
        Some((index, separator_len)) => (&output[..index], output[index + separator_len..].to_vec()),
        None => {
            let mut response = Response::new(200, output.to_vec());
            response.set_header("Content-Type", "text/plain; charset=utf-8");
            return response;
        }
    };

    let header_text = String::from_utf8_lossy(header_bytes);
    let mut status = 200;
    let mut response_headers = Vec::new();

    for line in header_text.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let trimmed_name = name.trim();
        let trimmed_value = value.trim();
        if trimmed_name.eq_ignore_ascii_case("Status") {
            if let Some(code) = trimmed_value.split_whitespace().next() {
                if let Ok(parsed_status) = code.parse::<u16>() {
                    status = parsed_status;
                }
            }
        } else {
            response_headers.push((trimmed_name.to_string(), trimmed_value.to_string()));
        }
    }

    let mut response = Response::new(status, body);
    for (name, value) in response_headers {
        response.set_header(name, value);
    }
    if !response.has_header("Content-Type") {
        response.set_header("Content-Type", "text/plain; charset=utf-8");
    }
    response
}

fn find_header_end(buffer: &[u8]) -> Option<(usize, usize)> {
    for index in 0..buffer.len().saturating_sub(3) {
        if &buffer[index..index + 4] == b"\r\n\r\n" {
            return Some((index, 4));
        }
    }
    for index in 0..buffer.len().saturating_sub(1) {
        if &buffer[index..index + 2] == b"\n\n" {
            return Some((index, 2));
        }
    }
    None
}
