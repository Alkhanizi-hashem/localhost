use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::config::{RouteConfig, ServerConfig};
use crate::http::{status_text, Method, Request, Response};
use crate::server::SessionInfo;
use crate::util::{content_type, html_escape, now_millis, sanitize_filename};

pub enum HandlerResult {
    Response(Response),
    Cgi {
        script: PathBuf,
        interpreter: PathBuf,
    },
}

impl From<Response> for HandlerResult {
    fn from(response: Response) -> Self {
        Self::Response(response)
    }
}

pub fn handle_request(
    request: &Request,
    server: &ServerConfig,
    session: &SessionInfo,
) -> HandlerResult {
    if request.body.len() > server.client_max_body_size {
        return error_response(413, server).into();
    }

    if request.path == "/session" {
        return session_response(session).into();
    }

    let Some(route) = find_route(server, &request.path) else {
        return error_response(404, server).into();
    };

    if !method_allowed(route, &request.method) {
        let mut response = error_response(405, server);
        response.set_header("Allow", route.allowed_methods_header());
        return response.into();
    }

    if let Some(redirect) = &route.redirect {
        let mut response = Response::html(
            redirect.status,
            format!(
                "<!doctype html><title>{}</title><h1>{}</h1><p>Redirecting to <a href=\"{}\">{}</a>.</p>",
                status_text(redirect.status),
                status_text(redirect.status),
                html_escape(&redirect.location),
                html_escape(&redirect.location)
            ),
        );
        response.set_header("Location", redirect.location.clone());
        return response.into();
    }

    match request.method {
        Method::Get => handle_get(request, route, server),
        Method::Post => handle_post(request, route, server),
        Method::Delete => handle_delete(request, route, server).into(),
        Method::Other(_) => {
            let mut response = error_response(405, server);
            response.set_header("Allow", route.allowed_methods_header());
            response.into()
        }
    }
}

fn find_route<'a>(server: &'a ServerConfig, path: &str) -> Option<&'a RouteConfig> {
    server.routes.iter().find(|route| {
        if route.path == "/" {
            return true;
        }
        path == route.path
            || path
                .strip_prefix(&route.path)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

fn method_allowed(route: &RouteConfig, method: &Method) -> bool {
    route.methods.is_empty() || route.methods.contains(method)
}

fn handle_get(request: &Request, route: &RouteConfig, server: &ServerConfig) -> HandlerResult {
    let target = match route_file_path(route, &request.path) {
        Ok(path) => path,
        Err(status) => return error_response(status, server).into(),
    };

    let target = match protect_existing_path(route, &target) {
        Ok(path) => path,
        Err(status) => return error_response(status, server).into(),
    };

    if target.is_dir() {
        return handle_directory_get(request, route, server, &target).into();
    }

    if !target.exists() {
        return error_response(404, server).into();
    }
    if !target.is_file() {
        return error_response(403, server).into();
    }

    if let Some(interpreter) = cgi_interpreter(route, &target) {
        return HandlerResult::Cgi {
            script: target,
            interpreter: interpreter.clone(),
        };
    }

    serve_file(&target, server).into()
}

fn handle_directory_get(
    request: &Request,
    route: &RouteConfig,
    server: &ServerConfig,
    directory: &Path,
) -> Response {
    if let Some(index) = &route.index {
        let index_path = directory.join(index);
        if index_path.is_file() {
            return serve_file(&index_path, server);
        }
    }

    if route.directory_listing {
        return directory_listing(request, directory, server);
    }

    error_response(403, server)
}

fn handle_post(request: &Request, route: &RouteConfig, server: &ServerConfig) -> HandlerResult {
    let target = match route_file_path(route, &request.path) {
        Ok(path) => path,
        Err(status) => return error_response(status, server).into(),
    };

    if target.exists() {
        match protect_existing_path(route, &target) {
            Ok(path) => {
                if let Some(interpreter) = cgi_interpreter(route, &path) {
                    return HandlerResult::Cgi {
                        script: path,
                        interpreter: interpreter.clone(),
                    };
                }
            }
            Err(status) => return error_response(status, server).into(),
        }
    }

    if let Some(upload_store) = &route.upload_store {
        return save_upload(request, upload_store, server).into();
    }

    let mut response = Response::text(200, format!("received {} bytes\n", request.body.len()));
    response.set_header("Cache-Control", "no-store");
    response.into()
}

fn handle_delete(request: &Request, route: &RouteConfig, server: &ServerConfig) -> Response {
    let target = match route_file_path(route, &request.path) {
        Ok(path) => path,
        Err(status) => return error_response(status, server),
    };

    let target = match protect_existing_path(route, &target) {
        Ok(path) => path,
        Err(status) => return error_response(status, server),
    };

    if !target.exists() {
        return error_response(404, server);
    }
    if target.is_dir() {
        return error_response(403, server);
    }

    match fs::remove_file(&target) {
        Ok(()) => Response::empty(204),
        Err(error) if error.kind() == io::ErrorKind::NotFound => error_response(404, server),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            error_response(403, server)
        }
        Err(_) => error_response(500, server),
    }
}

fn route_file_path(route: &RouteConfig, request_path: &str) -> Result<PathBuf, u16> {
    let root = route.root.as_ref().ok_or(404u16)?;

    if root.is_file() {
        return Ok(root.clone());
    }

    let remainder = if route.path == "/" {
        request_path.trim_start_matches('/')
    } else if request_path == route.path {
        ""
    } else {
        request_path
            .strip_prefix(&route.path)
            .unwrap_or("")
            .trim_start_matches('/')
    };

    let mut path = root.clone();
    for component in Path::new(remainder).components() {
        match component {
            Component::Normal(value) => path.push(value),
            Component::CurDir => {}
            Component::RootDir | Component::ParentDir | Component::Prefix(_) => return Err(403),
        }
    }
    Ok(path)
}

fn protect_existing_path(route: &RouteConfig, path: &Path) -> Result<PathBuf, u16> {
    if !path.exists() {
        return Ok(path.to_path_buf());
    }

    let root = route.root.as_ref().ok_or(404u16)?;
    let canonical_root = fs::canonicalize(root).map_err(|_| 404u16)?;
    let canonical_path = fs::canonicalize(path).map_err(|_| 404u16)?;
    if canonical_path.starts_with(canonical_root) {
        Ok(canonical_path)
    } else {
        Err(403)
    }
}

fn serve_file(path: &Path, server: &ServerConfig) -> Response {
    match fs::read(path) {
        Ok(body) => {
            let mut response = Response::new(200, body);
            response.set_header("Content-Type", content_type(path));
            response
        }
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            error_response(403, server)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => error_response(404, server),
        Err(_) => error_response(500, server),
    }
}

fn directory_listing(request: &Request, directory: &Path, server: &ServerConfig) -> Response {
    let mut entries = match fs::read_dir(directory) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let display_name = if entry.path().is_dir() {
                    format!("{name}/")
                } else {
                    name
                };
                display_name
            })
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            return error_response(403, server);
        }
        Err(_) => return error_response(500, server),
    };
    entries.sort();

    let mut body = String::from("<!doctype html><html><head><meta charset=\"utf-8\"><title>Directory listing</title></head><body>");
    body.push_str(&format!(
        "<h1>Index of {}</h1><ul>",
        html_escape(&request.path)
    ));
    if request.path != "/" {
        body.push_str("<li><a href=\"../\">../</a></li>");
    }
    for entry in entries {
        body.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>",
            html_escape(&entry),
            html_escape(&entry)
        ));
    }
    body.push_str("</ul></body></html>");
    Response::html(200, body)
}

fn save_upload(request: &Request, upload_store: &Path, server: &ServerConfig) -> Response {
    if let Err(error) = fs::create_dir_all(upload_store) {
        eprintln!("upload directory error: {error}");
        return error_response(500, server);
    }

    let saved = if let Some(content_type) = request.header("content-type") {
        if let Some(boundary) = multipart_boundary(content_type) {
            match save_multipart(&request.body, &boundary, upload_store) {
                Ok(paths) if !paths.is_empty() => paths,
                Ok(_) => return Response::text(400, "multipart request did not contain files\n"),
                Err(error) => {
                    eprintln!("multipart upload error: {error}");
                    return error_response(400, server);
                }
            }
        } else {
            match save_raw_body(request, upload_store) {
                Ok(path) => vec![path],
                Err(_) => return error_response(500, server),
            }
        }
    } else {
        match save_raw_body(request, upload_store) {
            Ok(path) => vec![path],
            Err(_) => return error_response(500, server),
        }
    };

    let mut body = String::from("<!doctype html><html><body><h1>Uploaded</h1><ul>");
    for path in saved {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("upload");
        body.push_str(&format!("<li>{}</li>", html_escape(name)));
    }
    body.push_str("</ul></body></html>");
    Response::html(201, body)
}

fn multipart_boundary(content_type: &str) -> Option<String> {
    content_type.split(';').find_map(|part| {
        let part = part.trim();
        part.strip_prefix("boundary=")
            .map(|value| value.trim_matches('"').to_string())
    })
}

fn save_multipart(body: &[u8], boundary: &str, upload_store: &Path) -> io::Result<Vec<PathBuf>> {
    let marker = format!("--{boundary}");
    let marker_bytes = marker.as_bytes();
    let mut saved = Vec::new();
    let mut position = 0;

    while let Some(marker_index) = find_bytes(&body[position..], marker_bytes) {
        position += marker_index + marker_bytes.len();
        if body[position..].starts_with(b"--") {
            break;
        }
        if body[position..].starts_with(b"\r\n") {
            position += 2;
        } else if body[position..].starts_with(b"\n") {
            position += 1;
        }

        let Some((header_end, separator_len)) = find_header_end(&body[position..]) else {
            break;
        };
        let headers = &body[position..position + header_end];
        position += header_end + separator_len;

        let next_marker =
            find_bytes(&body[position..], marker_bytes).unwrap_or(body.len() - position);
        let mut part_body = &body[position..position + next_marker];
        if part_body.ends_with(b"\r\n") {
            part_body = &part_body[..part_body.len() - 2];
        } else if part_body.ends_with(b"\n") {
            part_body = &part_body[..part_body.len() - 1];
        }
        position += next_marker;

        let header_text = String::from_utf8_lossy(headers);
        let Some(filename) = filename_from_part_headers(&header_text) else {
            continue;
        };
        let Some(safe_name) = sanitize_filename(&filename) else {
            continue;
        };
        let path = unique_upload_path(upload_store, &safe_name);
        fs::write(&path, part_body)?;
        saved.push(path);
    }

    Ok(saved)
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

fn filename_from_part_headers(headers: &str) -> Option<String> {
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("content-disposition") {
            continue;
        }
        for attribute in value.split(';') {
            let attribute = attribute.trim();
            if let Some(filename) = attribute.strip_prefix("filename=") {
                return Some(filename.trim_matches('"').to_string());
            }
        }
    }
    None
}

fn save_raw_body(request: &Request, upload_store: &Path) -> io::Result<PathBuf> {
    let requested_name = request
        .header("x-filename")
        .and_then(sanitize_filename)
        .unwrap_or_else(|| format!("upload-{}.bin", now_millis()));
    let path = unique_upload_path(upload_store, &requested_name);
    fs::write(&path, &request.body)?;
    Ok(path)
}

fn unique_upload_path(directory: &Path, filename: &str) -> PathBuf {
    let mut candidate = directory.join(filename);
    if !candidate.exists() {
        return candidate;
    }

    let stem = Path::new(filename)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("upload");
    let extension = Path::new(filename)
        .extension()
        .and_then(|value| value.to_str());
    for index in 1..10_000 {
        let name = match extension {
            Some(extension) => format!("{stem}-{index}.{extension}"),
            None => format!("{stem}-{index}"),
        };
        candidate = directory.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }

    directory.join(format!("upload-{}.bin", now_millis()))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn cgi_interpreter<'a>(route: &'a RouteConfig, path: &Path) -> Option<&'a PathBuf> {
    let extension = path.extension()?.to_str()?;
    let key = format!(".{extension}");
    route.cgi.get(&key)
}

fn session_response(session: &SessionInfo) -> Response {
    Response::html(
        200,
        format!(
            "<!doctype html><html><body><h1>Session</h1><p>id: {}</p><p>visits: {}</p></body></html>",
            html_escape(&session.id),
            session.visits
        ),
    )
}

pub fn error_response(status: u16, server: &ServerConfig) -> Response {
    if let Some(path) = server.error_pages.get(&status) {
        if let Ok(body) = fs::read(path) {
            let mut response = Response::new(status, body);
            response.set_header("Content-Type", content_type(path));
            return response;
        }
    }

    Response::html(
        status,
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>{status} {}</title></head><body><h1>{status} {}</h1></body></html>",
            status_text(status),
            status_text(status)
        ),
    )
}

#[allow(dead_code)]
fn _headers_to_map(headers: &[(String, String)]) -> HashMap<String, String> {
    headers
        .iter()
        .map(|(key, value)| (key.to_ascii_lowercase(), value.clone()))
        .collect()
}
