use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::ffi::set_nonblocking;
use crate::http::{Request, Response};

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

pub struct CgiProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    body: Vec<u8>,
    written: usize,
    output: Vec<u8>,
    deadline: Instant,
    exit_status: Option<ExitStatus>,
}

impl CgiProcess {
    pub fn spawn(
        request: &Request,
        script_path: &Path,
        interpreter: &Path,
        timeout: Duration,
    ) -> Result<Self, CgiError> {
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
                    .map(|ch| if ch == '-' {
                        '_'
                    } else {
                        ch.to_ascii_uppercase()
                    })
                    .collect::<String>()
            );
            command.env(env_name, value);
        }

        let mut child = command.spawn()?;
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "missing CGI stdin").into());
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "missing CGI stdout").into());
        };

        if let Err(error) =
            set_nonblocking(stdin.as_raw_fd()).and_then(|()| set_nonblocking(stdout.as_raw_fd()))
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
        let stdin = if request.body.is_empty() {
            drop(stdin);
            None
        } else {
            Some(stdin)
        };

        Ok(Self {
            child,
            stdin,
            stdout: Some(stdout),
            body: request.body.clone(),
            written: 0,
            output: Vec::new(),
            deadline: Instant::now() + timeout,
            exit_status: None,
        })
    }

    pub fn stdin_fd(&self) -> Option<RawFd> {
        self.stdin.as_ref().map(AsRawFd::as_raw_fd)
    }

    pub fn stdout_fd(&self) -> Option<RawFd> {
        self.stdout.as_ref().map(AsRawFd::as_raw_fd)
    }

    pub fn write_stdin_once(&mut self) -> Result<bool, CgiError> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Ok(true);
        };

        match stdin.write(&self.body[self.written..]) {
            Ok(0) => return Ok(false),
            Ok(bytes) => self.written += bytes,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(false),
            Err(error) => return Err(CgiError::Io(error)),
        }

        if self.written >= self.body.len() {
            self.stdin.take();
            return Ok(true);
        }
        Ok(false)
    }

    pub fn close_stdin(&mut self) {
        self.stdin.take();
    }

    pub fn read_stdout_once(&mut self) -> Result<bool, CgiError> {
        let Some(stdout) = self.stdout.as_mut() else {
            return Ok(true);
        };
        let mut buffer = [0u8; CGI_BUFFER_SIZE];
        match stdout.read(&mut buffer) {
            Ok(0) => {
                self.stdout.take();
                Ok(true)
            }
            Ok(bytes) => {
                self.output.extend_from_slice(&buffer[..bytes]);
                Ok(false)
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(false),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(false),
            Err(error) => Err(CgiError::Io(error)),
        }
    }

    pub fn poll(&mut self) -> Option<Result<Response, CgiError>> {
        if Instant::now() >= self.deadline {
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.exit_status = self.child.try_wait().ok().flatten();
            return Some(Err(CgiError::Timeout));
        }

        if self.exit_status.is_none() {
            match self.child.try_wait() {
                Ok(status) => self.exit_status = status,
                Err(error) => return Some(Err(CgiError::Io(error))),
            }
        }

        if self.stdout.is_some() || self.exit_status.is_none() {
            return None;
        }
        if self.exit_status.as_ref().is_some_and(ExitStatus::success) {
            Some(Ok(parse_output(&self.output)))
        } else {
            Some(Err(CgiError::InvalidOutput))
        }
    }
}

impl Drop for CgiProcess {
    fn drop(&mut self) {
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }
}

fn parse_output(output: &[u8]) -> Response {
    let (header_bytes, body) = match find_header_end(output) {
        Some((index, separator_len)) => {
            (&output[..index], output[index + separator_len..].to_vec())
        }
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
        } else if !matches!(
            trimmed_name.to_ascii_lowercase().as_str(),
            "connection" | "content-length" | "server" | "transfer-encoding"
        ) {
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
