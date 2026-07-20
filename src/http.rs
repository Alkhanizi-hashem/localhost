use std::collections::HashMap;
use std::fmt;

use crate::util::{host_without_port, percent_decode};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Delete,
    Other(String),
}

impl Method {
    pub fn from_token(token: &str) -> Self {
        match token {
            "GET" => Self::Get,
            "POST" => Self::Post,
            "DELETE" => Self::Delete,
            other => Self::Other(other.to_string()),
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Get => write!(formatter, "GET"),
            Self::Post => write!(formatter, "POST"),
            Self::Delete => write!(formatter, "DELETE"),
            Self::Other(value) => write!(formatter, "{value}"),
        }
    }
}

#[derive(Debug)]
pub struct Request {
    pub method: Method,
    pub target: String,
    pub path: String,
    pub query: String,
    pub version: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn host(&self) -> Option<String> {
        self.header("host").map(host_without_port)
    }

    pub fn should_close_connection(&self) -> bool {
        let connection = self
            .header("connection")
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_default();

        if self.version == "HTTP/1.0" {
            !connection
                .split(',')
                .any(|value| value.trim() == "keep-alive")
        } else {
            connection.split(',').any(|value| value.trim() == "close")
        }
    }
}

pub enum ParseResult {
    Complete(Request, usize),
    Incomplete,
    BadRequest(String),
    TooLarge,
}

pub fn parse_request(buffer: &[u8], max_body_size: usize) -> ParseResult {
    let (header_end, separator_len) = match find_header_end(buffer) {
        Some(result) => result,
        None => {
            if buffer.len() > 16 * 1024 {
                return ParseResult::BadRequest("request headers are too large".to_string());
            }
            return ParseResult::Incomplete;
        }
    };

    let header_bytes = &buffer[..header_end];
    let header_text = match std::str::from_utf8(header_bytes) {
        Ok(value) => value,
        Err(_) => return ParseResult::BadRequest("headers are not valid UTF-8".to_string()),
    };

    let mut lines = header_text.lines().map(str::trim_end);
    let start_line = match lines.next() {
        Some(line) if !line.is_empty() => line,
        _ => return ParseResult::BadRequest("missing request line".to_string()),
    };

    let mut start_parts = start_line.split_whitespace();
    let method = match start_parts.next() {
        Some(value) => Method::from_token(value),
        None => return ParseResult::BadRequest("missing method".to_string()),
    };
    let target = match start_parts.next() {
        Some(value) => value.to_string(),
        None => return ParseResult::BadRequest("missing target".to_string()),
    };
    let version = match start_parts.next() {
        Some("HTTP/1.0") => "HTTP/1.0".to_string(),
        Some("HTTP/1.1") => "HTTP/1.1".to_string(),
        Some(other) => return ParseResult::BadRequest(format!("unsupported HTTP version {other}")),
        None => return ParseResult::BadRequest("missing HTTP version".to_string()),
    };
    if start_parts.next().is_some() {
        return ParseResult::BadRequest("request line has too many fields".to_string());
    }

    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return ParseResult::BadRequest(format!("invalid header line {line}"));
        };
        let normalized_name = name.trim().to_ascii_lowercase();
        if normalized_name.is_empty() {
            return ParseResult::BadRequest("empty header name".to_string());
        }
        let normalized_value = value.trim().to_string();
        headers
            .entry(normalized_name)
            .and_modify(|existing: &mut String| {
                existing.push_str(", ");
                existing.push_str(&normalized_value);
            })
            .or_insert(normalized_value);
    }

    if version == "HTTP/1.1" && !headers.contains_key("host") {
        return ParseResult::BadRequest("HTTP/1.1 requests require a Host header".to_string());
    }

    let body_start = header_end + separator_len;
    let body_bytes = &buffer[body_start..];
    let transfer_encoding = headers
        .get("transfer-encoding")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();

    let (body, consumed_body_bytes) = if transfer_encoding
        .split(',')
        .any(|value| value.trim() == "chunked")
    {
        match decode_chunked(body_bytes, max_body_size) {
            Ok(Some(result)) => result,
            Ok(None) => return ParseResult::Incomplete,
            Err(ChunkError::BadRequest(message)) => return ParseResult::BadRequest(message),
            Err(ChunkError::TooLarge) => return ParseResult::TooLarge,
        }
    } else {
        let content_length = match headers.get("content-length") {
            Some(value) => match value.parse::<usize>() {
                Ok(length) => length,
                Err(_) => return ParseResult::BadRequest("invalid Content-Length".to_string()),
            },
            None => 0,
        };
        if content_length > max_body_size {
            return ParseResult::TooLarge;
        }
        if body_bytes.len() < content_length {
            return ParseResult::Incomplete;
        }
        (body_bytes[..content_length].to_vec(), content_length)
    };

    let (path, query) = match parse_target(&target) {
        Ok(result) => result,
        Err(message) => return ParseResult::BadRequest(message),
    };

    ParseResult::Complete(
        Request {
            method,
            target,
            path,
            query,
            version,
            headers,
            body,
        },
        body_start + consumed_body_bytes,
    )
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

fn parse_target(target: &str) -> Result<(String, String), String> {
    let without_scheme = if let Some(rest) = target.strip_prefix("http://") {
        match rest.find('/') {
            Some(index) => &rest[index..],
            None => "/",
        }
    } else {
        target
    };

    let (raw_path, query) = match without_scheme.split_once('?') {
        Some((path, query)) => (path, query),
        None => (without_scheme, ""),
    };

    if raw_path.is_empty() || !raw_path.starts_with('/') {
        return Err("request target must be an absolute path".to_string());
    }

    let decoded_path = percent_decode(raw_path)
        .ok_or_else(|| "request target contains invalid percent encoding".to_string())?;
    Ok((decoded_path, query.to_string()))
}

enum ChunkError {
    BadRequest(String),
    TooLarge,
}

fn decode_chunked(
    buffer: &[u8],
    max_body_size: usize,
) -> Result<Option<(Vec<u8>, usize)>, ChunkError> {
    let mut position = 0;
    let mut body = Vec::new();

    loop {
        let Some((line_end, separator_len)) = find_line_end(&buffer[position..]) else {
            return Ok(None);
        };
        let size_line = &buffer[position..position + line_end];
        let size_text = std::str::from_utf8(size_line)
            .map_err(|_| ChunkError::BadRequest("invalid chunk size".to_string()))?;
        let size_token = size_text.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_token, 16)
            .map_err(|_| ChunkError::BadRequest("invalid chunk size".to_string()))?;
        position += line_end + separator_len;

        if size == 0 {
            if buffer.len() < position {
                return Ok(None);
            }
            if buffer[position..].starts_with(b"\r\n") {
                return Ok(Some((body, position + 2)));
            }
            if buffer[position..].starts_with(b"\n") {
                return Ok(Some((body, position + 1)));
            }
            if let Some((trailer_end, trailer_separator_len)) = find_header_end(&buffer[position..])
            {
                return Ok(Some((body, position + trailer_end + trailer_separator_len)));
            }
            return Ok(None);
        }

        if body.len().saturating_add(size) > max_body_size {
            return Err(ChunkError::TooLarge);
        }
        if buffer.len() < position + size + 1 {
            return Ok(None);
        }

        body.extend_from_slice(&buffer[position..position + size]);
        position += size;

        if buffer[position..].starts_with(b"\r\n") {
            position += 2;
        } else if buffer[position..].starts_with(b"\n") {
            position += 1;
        } else {
            return Err(ChunkError::BadRequest(
                "chunk data is not followed by a newline".to_string(),
            ));
        }
    }
}

fn find_line_end(buffer: &[u8]) -> Option<(usize, usize)> {
    for index in 0..buffer.len() {
        if buffer[index] == b'\n' {
            let line_end = if index > 0 && buffer[index - 1] == b'\r' {
                index - 1
            } else {
                index
            };
            return Some((line_end, index + 1 - line_end));
        }
    }
    None
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body,
        }
    }

    pub fn text(status: u16, body: impl Into<String>) -> Self {
        let mut response = Self::new(status, body.into().into_bytes());
        response.set_header("Content-Type", "text/plain; charset=utf-8");
        response
    }

    pub fn html(status: u16, body: impl Into<String>) -> Self {
        let mut response = Self::new(status, body.into().into_bytes());
        response.set_header("Content-Type", "text/html; charset=utf-8");
        response
    }

    pub fn empty(status: u16) -> Self {
        Self::new(status, Vec::new())
    }

    pub fn set_header(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let name_lower = name.to_ascii_lowercase();
        if let Some((_, existing_value)) = self
            .headers
            .iter_mut()
            .find(|(existing_name, _)| existing_name.to_ascii_lowercase() == name_lower)
        {
            *existing_value = value.into();
        } else {
            self.headers.push((name, value.into()));
        }
    }

    pub fn has_header(&self, name: &str) -> bool {
        let name_lower = name.to_ascii_lowercase();
        self.headers
            .iter()
            .any(|(existing_name, _)| existing_name.to_ascii_lowercase() == name_lower)
    }

    pub fn to_bytes(&self, close_connection: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        let status_line = format!("HTTP/1.1 {} {}\r\n", self.status, status_text(self.status));
        bytes.extend_from_slice(status_line.as_bytes());
        bytes.extend_from_slice(b"Server: localhost-rust\r\n");
        bytes.extend_from_slice(format!("Content-Length: {}\r\n", self.body.len()).as_bytes());

        if !self.has_header("Content-Type") && self.status != 204 {
            bytes.extend_from_slice(b"Content-Type: application/octet-stream\r\n");
        }

        if close_connection {
            bytes.extend_from_slice(b"Connection: close\r\n");
        } else {
            bytes.extend_from_slice(b"Connection: keep-alive\r\n");
        }

        for (name, value) in &self.headers {
            bytes.extend_from_slice(name.as_bytes());
            bytes.extend_from_slice(b": ");
            bytes.extend_from_slice(value.as_bytes());
            bytes.extend_from_slice(b"\r\n");
        }

        bytes.extend_from_slice(b"\r\n");
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

pub fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        504 => "Gateway Timeout",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_request, ParseResult};

    #[test]
    fn parses_content_length_request() {
        let raw = b"POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhello";
        let ParseResult::Complete(request, consumed) = parse_request(raw, 1024) else {
            panic!("request should parse");
        };
        assert_eq!(request.path, "/upload");
        assert_eq!(request.body, b"hello");
        assert_eq!(consumed, raw.len());
    }

    #[test]
    fn parses_chunked_request() {
        let raw = b"POST /cgi/echo.py HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        let ParseResult::Complete(request, _) = parse_request(raw, 1024) else {
            panic!("request should parse");
        };
        assert_eq!(request.body, b"hello");
    }
}
