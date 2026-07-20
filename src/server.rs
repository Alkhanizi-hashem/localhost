use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::{AsRawFd, RawFd};
use std::panic::{self, AssertUnwindSafe};
use std::time::{Duration, Instant};

use crate::cgi::{CgiError, CgiProcess};
use crate::config::{Config, ServerConfig};
use crate::epoll::Epoll;
use crate::ffi::{set_nonblocking, EpollEvent, EPOLLERR, EPOLLHUP, EPOLLIN, EPOLLOUT, EPOLLRDHUP};
use crate::http::{parse_request, ParseResult, Request, Response};
use crate::router::{error_response, handle_request, HandlerResult};
use crate::util::{host_without_port, now_millis};

const CLIENT_READ_EVENTS: u32 = EPOLLIN | EPOLLERR | EPOLLHUP | EPOLLRDHUP;
const CLIENT_READ_WRITE_EVENTS: u32 = EPOLLIN | EPOLLOUT | EPOLLERR | EPOLLHUP | EPOLLRDHUP;
const CLIENT_WAIT_EVENTS: u32 = EPOLLERR | EPOLLHUP | EPOLLRDHUP;
const LISTENER_EVENTS: u32 = EPOLLIN | EPOLLERR | EPOLLHUP;
const CGI_STDOUT_EVENTS: u32 = EPOLLIN | EPOLLERR | EPOLLHUP | EPOLLRDHUP;
const CGI_STDIN_EVENTS: u32 = EPOLLOUT | EPOLLERR | EPOLLHUP | EPOLLRDHUP;
const READ_BUFFER_SIZE: usize = 16 * 1024;
const SESSION_COOKIE: &str = "LOCALHOST_SESSION";

#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub id: String,
    pub visits: u64,
    pub is_new: bool,
}

struct Session {
    last_seen: Instant,
    visits: u64,
}

struct ListenGroup {
    listener: TcpListener,
    host: String,
    port: u16,
    servers: Vec<usize>,
    max_body_size: usize,
}

struct Client {
    stream: TcpStream,
    listen_fd: RawFd,
    read_buffer: Vec<u8>,
    write_buffer: Vec<u8>,
    write_position: usize,
    close_after_write: bool,
    max_body_size: usize,
    last_active: Instant,
    waiting_cgi: Option<u64>,
}

#[derive(Clone, Copy)]
enum CgiPipe {
    Stdin(u64),
    Stdout(u64),
}

impl CgiPipe {
    fn job_id(self) -> u64 {
        match self {
            Self::Stdin(job_id) | Self::Stdout(job_id) => job_id,
        }
    }
}

struct CgiJob {
    process: CgiProcess,
    client_fd: RawFd,
    server: ServerConfig,
    session: SessionInfo,
    close_after_write: bool,
    http10: bool,
}

pub struct Server {
    config: Config,
    epoll: Epoll,
    listeners: HashMap<RawFd, ListenGroup>,
    clients: HashMap<RawFd, Client>,
    sessions: HashMap<String, Session>,
    session_counter: u64,
    cgi_jobs: HashMap<u64, CgiJob>,
    cgi_pipes: HashMap<RawFd, CgiPipe>,
    cgi_counter: u64,
}

impl Server {
    pub fn new(config: Config) -> io::Result<(Self, Vec<String>)> {
        let epoll = Epoll::new()?;
        let mut server = Self {
            config,
            epoll,
            listeners: HashMap::new(),
            clients: HashMap::new(),
            sessions: HashMap::new(),
            session_counter: 0,
            cgi_jobs: HashMap::new(),
            cgi_pipes: HashMap::new(),
            cgi_counter: 0,
        };

        let warnings = server.bind_listeners()?;
        if server.listeners.is_empty() {
            let detail = if warnings.is_empty() {
                "no valid listeners could be created".to_string()
            } else {
                format!(
                    "no valid listeners could be created: {}",
                    warnings.join("; ")
                )
            };
            return Err(io::Error::new(io::ErrorKind::InvalidInput, detail));
        }

        Ok((server, warnings))
    }

    pub fn run(&mut self) -> io::Result<()> {
        let listeners = self
            .listeners
            .values()
            .map(|listener| format!("{}:{}", listener.host, listener.port))
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!("listening on {listeners}");

        let mut events = vec![EpollEvent::empty(); 256];
        loop {
            let ready = self.epoll.wait(&mut events, 100)?;
            for event in events.iter().take(ready) {
                let fd = event.data as RawFd;
                if self.listeners.contains_key(&fd) {
                    if let Err(_panic) =
                        panic::catch_unwind(AssertUnwindSafe(|| self.accept_clients(fd)))
                    {
                        eprintln!("listener handler panicked on fd {fd}");
                    }
                } else if self.clients.contains_key(&fd) {
                    if let Err(_panic) = panic::catch_unwind(AssertUnwindSafe(|| {
                        self.handle_client_event(fd, event.events)
                    })) {
                        eprintln!("client handler panicked on fd {fd}");
                        self.close_client(fd);
                    }
                } else if self.cgi_pipes.contains_key(&fd) {
                    if let Err(_panic) = panic::catch_unwind(AssertUnwindSafe(|| {
                        self.handle_cgi_event(fd, event.events)
                    })) {
                        eprintln!("CGI handler panicked on fd {fd}");
                        if let Some(pipe) = self.cgi_pipes.get(&fd).copied() {
                            self.finish_cgi_job(pipe.job_id(), Err(CgiError::InvalidOutput));
                        }
                    }
                }
            }
            if let Err(_panic) = panic::catch_unwind(AssertUnwindSafe(|| self.poll_cgi_jobs())) {
                eprintln!("CGI cleanup panicked");
            }
            if let Err(_panic) =
                panic::catch_unwind(AssertUnwindSafe(|| self.close_timed_out_clients()))
            {
                eprintln!("timeout cleanup panicked");
            }
            if let Err(_panic) = panic::catch_unwind(AssertUnwindSafe(|| self.expire_sessions())) {
                eprintln!("session cleanup panicked");
            }
        }
    }

    fn bind_listeners(&mut self) -> io::Result<Vec<String>> {
        let mut warnings = Vec::new();
        let mut endpoints: HashMap<(String, u16), EndpointConfig> = HashMap::new();

        for (server_index, server) in self.config.servers.iter().enumerate() {
            for port in &server.ports {
                let key = (server.host.clone(), *port);
                let endpoint = endpoints.entry(key).or_default();
                if server.server_names.is_empty() {
                    if endpoint.has_unnamed_default {
                        warnings.push(format!(
                            "ignored duplicate unnamed default server on {}:{}",
                            server.host, port
                        ));
                        continue;
                    }
                    endpoint.has_unnamed_default = true;
                }

                let mut duplicate_name = None;
                for name in &server.server_names {
                    if !endpoint.server_names.insert(name.clone()) {
                        duplicate_name = Some(name.clone());
                        break;
                    }
                }
                if let Some(name) = duplicate_name {
                    warnings.push(format!(
                        "ignored server {}:{} because server_name {name} is duplicated",
                        server.host, port
                    ));
                    continue;
                }

                endpoint.servers.push(server_index);
            }
        }

        for ((host, port), endpoint) in endpoints {
            if endpoint.servers.is_empty() {
                continue;
            }

            let address = format!("{host}:{port}");
            let listener = match TcpListener::bind(&address) {
                Ok(listener) => listener,
                Err(error) => {
                    warnings.push(format!("could not bind {address}: {error}"));
                    continue;
                }
            };
            listener.set_nonblocking(true)?;
            let fd = listener.as_raw_fd();
            set_nonblocking(fd)?;
            self.epoll.add(fd, LISTENER_EVENTS)?;

            let max_body_size = endpoint
                .servers
                .iter()
                .filter_map(|index| self.config.servers.get(*index))
                .map(|server| server.client_max_body_size)
                .max()
                .unwrap_or(1_048_576);

            self.listeners.insert(
                fd,
                ListenGroup {
                    listener,
                    host,
                    port,
                    servers: endpoint.servers,
                    max_body_size,
                },
            );
        }

        Ok(warnings)
    }

    fn accept_clients(&mut self, listen_fd: RawFd) {
        loop {
            let Some(listener) = self.listeners.get(&listen_fd) else {
                return;
            };

            match listener.listener.accept() {
                Ok((stream, _address)) => {
                    if let Err(error) = self.register_client(listen_fd, stream) {
                        eprintln!("failed to register client: {error}");
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    eprintln!("accept error: {error}");
                    break;
                }
            }
        }
    }

    fn register_client(&mut self, listen_fd: RawFd, stream: TcpStream) -> io::Result<()> {
        stream.set_nonblocking(true)?;
        let fd = stream.as_raw_fd();
        set_nonblocking(fd)?;
        self.epoll.add(fd, CLIENT_READ_EVENTS)?;
        let max_body_size = self
            .listeners
            .get(&listen_fd)
            .map(|listener| listener.max_body_size)
            .unwrap_or(1_048_576);

        self.clients.insert(
            fd,
            Client {
                stream,
                listen_fd,
                read_buffer: Vec::with_capacity(READ_BUFFER_SIZE),
                write_buffer: Vec::new(),
                write_position: 0,
                close_after_write: false,
                max_body_size,
                last_active: Instant::now(),
                waiting_cgi: None,
            },
        );
        Ok(())
    }

    fn handle_client_event(&mut self, fd: RawFd, events: u32) {
        if events & (EPOLLERR | EPOLLHUP) != 0 {
            self.close_client(fd);
            return;
        }

        let has_response = self
            .clients
            .get(&fd)
            .map(|client| !client.write_buffer.is_empty())
            .unwrap_or(false);

        if has_response {
            if events & EPOLLOUT != 0 {
                self.write_client(fd);
                if !self.clients.contains_key(&fd) {
                    return;
                }
            }
        } else if events & EPOLLIN != 0 {
            self.read_client(fd);
            if !self.clients.contains_key(&fd) {
                return;
            }
        }

        if events & EPOLLRDHUP != 0 {
            self.close_client(fd);
        }
    }

    fn read_client(&mut self, fd: RawFd) {
        let mut buffer = [0u8; READ_BUFFER_SIZE];
        let read_result = match self.clients.get_mut(&fd) {
            Some(client) => {
                client.last_active = Instant::now();
                client.stream.read(&mut buffer)
            }
            None => return,
        };

        match read_result {
            Ok(0) => {
                self.close_client(fd);
            }
            Ok(bytes_read) => {
                if let Some(client) = self.clients.get_mut(&fd) {
                    client.read_buffer.extend_from_slice(&buffer[..bytes_read]);
                }
                self.try_build_response(fd);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                eprintln!("read error: {error}");
                self.close_client(fd);
            }
        }
    }

    fn try_build_response(&mut self, fd: RawFd) {
        let parse_result = match self.clients.get(&fd) {
            Some(client) => parse_request(&client.read_buffer, client.max_body_size),
            None => return,
        };

        let response = match parse_result {
            ParseResult::Complete(request, consumed) => {
                if let Some(client) = self.clients.get_mut(&fd) {
                    client.read_buffer.drain(..consumed);
                }
                let Some(response) = self.response_for_request(fd, request) else {
                    return;
                };
                response
            }
            ParseResult::Incomplete => return,
            ParseResult::BadRequest(message) => {
                eprintln!("bad request: {message}");
                (self.response_for_bad_request(fd, 400), true)
            }
            ParseResult::TooLarge => (self.response_for_bad_request(fd, 413), true),
        };

        self.queue_response(fd, response.0, response.1);
    }

    fn response_for_request(&mut self, fd: RawFd, request: Request) -> Option<(Response, bool)> {
        let Some(server_index) = self.select_server_index(fd, &request) else {
            return Some((
                Response::text(500, "server configuration not found\n"),
                true,
            ));
        };
        let server = self.config.servers[server_index].clone();
        let close_after_write = request.should_close_connection();
        let http10 = request.version == "HTTP/1.0";
        let session = self.touch_session(&request);
        match handle_request(&request, &server, &session) {
            HandlerResult::Response(mut response) => {
                Self::decorate_response(&mut response, &session, close_after_write, http10);
                Some((response, close_after_write))
            }
            HandlerResult::Cgi {
                script,
                interpreter,
            } => {
                match CgiProcess::spawn(&request, &script, &interpreter, self.config.cgi_timeout) {
                    Ok(process) => {
                        if let Err(error) = self.start_cgi_job(
                            fd,
                            process,
                            server.clone(),
                            session.clone(),
                            close_after_write,
                            http10,
                        ) {
                            eprintln!("failed to register CGI process: {error}");
                            let mut response = error_response(500, &server);
                            Self::decorate_response(
                                &mut response,
                                &session,
                                close_after_write,
                                http10,
                            );
                            Some((response, close_after_write))
                        } else {
                            None
                        }
                    }
                    Err(CgiError::Io(error)) => {
                        eprintln!("cgi I/O error: {error}");
                        let mut response = error_response(500, &server);
                        Self::decorate_response(&mut response, &session, close_after_write, http10);
                        Some((response, close_after_write))
                    }
                    Err(CgiError::Timeout | CgiError::InvalidOutput) => {
                        let mut response = error_response(500, &server);
                        Self::decorate_response(&mut response, &session, close_after_write, http10);
                        Some((response, close_after_write))
                    }
                }
            }
        }
    }

    fn decorate_response(
        response: &mut Response,
        session: &SessionInfo,
        close_after_write: bool,
        http10: bool,
    ) {
        if session.is_new {
            response.set_header(
                "Set-Cookie",
                format!(
                    "{SESSION_COOKIE}={}; Path=/; HttpOnly; SameSite=Lax",
                    session.id
                ),
            );
        }
        response.set_header("X-Session-Visits", session.visits.to_string());
        if close_after_write {
            response.set_header("Connection", "close");
        } else if http10 {
            response.set_header("Connection", "keep-alive");
        }
    }

    fn response_for_bad_request(&mut self, fd: RawFd, status: u16) -> Response {
        let server = self
            .listeners
            .get(
                &self
                    .clients
                    .get(&fd)
                    .map(|client| client.listen_fd)
                    .unwrap_or(fd),
            )
            .and_then(|listener| listener.servers.first())
            .and_then(|index| self.config.servers.get(*index))
            .cloned();

        match server {
            Some(server) => error_response(status, &server),
            None => Response::text(
                status,
                format!("{} {}\n", status, crate::http::status_text(status)),
            ),
        }
    }

    fn queue_response(&mut self, fd: RawFd, response: Response, close_after_write: bool) {
        let bytes = response.to_bytes(close_after_write);
        if let Some(client) = self.clients.get_mut(&fd) {
            client.write_buffer = bytes;
            client.write_position = 0;
            client.close_after_write = close_after_write;
            client.last_active = Instant::now();
        }
        if let Err(error) = self.epoll.modify(fd, CLIENT_READ_WRITE_EVENTS) {
            eprintln!("failed to enable write notifications: {error}");
            self.close_client(fd);
        }
    }

    fn write_client(&mut self, fd: RawFd) {
        let write_result = match self.clients.get_mut(&fd) {
            Some(client) => {
                if client.write_position >= client.write_buffer.len() {
                    if let Err(error) = self.epoll.modify(fd, CLIENT_READ_EVENTS) {
                        eprintln!("failed to disable write notifications: {error}");
                        self.close_client(fd);
                    }
                    return;
                }
                client.last_active = Instant::now();
                client
                    .stream
                    .write(&client.write_buffer[client.write_position..])
            }
            None => return,
        };

        match write_result {
            Ok(0) => {
                self.close_client(fd);
            }
            Ok(bytes_written) => {
                let done = if let Some(client) = self.clients.get_mut(&fd) {
                    client.write_position += bytes_written;
                    client.write_position >= client.write_buffer.len()
                } else {
                    false
                };
                if done {
                    let mut should_close = true;
                    if let Some(client) = self.clients.get_mut(&fd) {
                        should_close = client.close_after_write;
                        client.write_buffer.clear();
                        client.write_position = 0;
                        client.close_after_write = false;
                    }

                    if should_close {
                        self.close_client(fd);
                    } else {
                        if let Err(error) = self.epoll.modify(fd, CLIENT_READ_EVENTS) {
                            eprintln!("failed to disable write notifications: {error}");
                            self.close_client(fd);
                            return;
                        }
                        self.try_build_response(fd);
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                eprintln!("write error: {error}");
                self.close_client(fd);
            }
        }
    }

    fn start_cgi_job(
        &mut self,
        client_fd: RawFd,
        process: CgiProcess,
        server: ServerConfig,
        session: SessionInfo,
        close_after_write: bool,
        http10: bool,
    ) -> io::Result<()> {
        self.cgi_counter = self.cgi_counter.saturating_add(1);
        let job_id = self.cgi_counter;
        let stdin_fd = process.stdin_fd();
        let stdout_fd = process.stdout_fd();

        if let Some(fd) = stdout_fd {
            self.epoll.add(fd, CGI_STDOUT_EVENTS)?;
        }
        if let Some(fd) = stdin_fd {
            if let Err(error) = self.epoll.add(fd, CGI_STDIN_EVENTS) {
                if let Some(stdout_fd) = stdout_fd {
                    self.epoll.delete(stdout_fd);
                }
                return Err(error);
            }
        }
        if let Err(error) = self.epoll.modify(client_fd, CLIENT_WAIT_EVENTS) {
            if let Some(fd) = stdin_fd {
                self.epoll.delete(fd);
            }
            if let Some(fd) = stdout_fd {
                self.epoll.delete(fd);
            }
            return Err(error);
        }

        if let Some(fd) = stdin_fd {
            self.cgi_pipes.insert(fd, CgiPipe::Stdin(job_id));
        }
        if let Some(fd) = stdout_fd {
            self.cgi_pipes.insert(fd, CgiPipe::Stdout(job_id));
        }
        self.cgi_jobs.insert(
            job_id,
            CgiJob {
                process,
                client_fd,
                server,
                session,
                close_after_write,
                http10,
            },
        );
        if let Some(client) = self.clients.get_mut(&client_fd) {
            client.waiting_cgi = Some(job_id);
            client.last_active = Instant::now();
        }
        Ok(())
    }

    fn handle_cgi_event(&mut self, fd: RawFd, events: u32) {
        let Some(pipe) = self.cgi_pipes.get(&fd).copied() else {
            return;
        };
        let job_id = pipe.job_id();
        let result = match (pipe, self.cgi_jobs.get_mut(&job_id)) {
            (CgiPipe::Stdin(_), Some(job)) => {
                if events & (EPOLLERR | EPOLLHUP | EPOLLRDHUP) != 0 {
                    job.process.close_stdin();
                    Ok(true)
                } else if events & EPOLLOUT != 0 {
                    job.process.write_stdin_once()
                } else {
                    Ok(false)
                }
            }
            (CgiPipe::Stdout(_), Some(job)) if events & CGI_STDOUT_EVENTS != 0 => {
                job.process.read_stdout_once()
            }
            _ => return,
        };

        match result {
            Ok(true) => {
                self.epoll.delete(fd);
                self.cgi_pipes.remove(&fd);
            }
            Ok(false) => {}
            Err(error) => {
                self.finish_cgi_job(job_id, Err(error));
                return;
            }
        }
        self.poll_cgi_job(job_id);
    }

    fn poll_cgi_jobs(&mut self) {
        let job_ids = self.cgi_jobs.keys().copied().collect::<Vec<_>>();
        for job_id in job_ids {
            self.poll_cgi_job(job_id);
        }
    }

    fn poll_cgi_job(&mut self, job_id: u64) {
        let result = self
            .cgi_jobs
            .get_mut(&job_id)
            .and_then(|job| job.process.poll());
        if let Some(result) = result {
            self.finish_cgi_job(job_id, result);
        }
    }

    fn finish_cgi_job(&mut self, job_id: u64, result: Result<Response, CgiError>) {
        let Some(job) = self.cgi_jobs.remove(&job_id) else {
            return;
        };
        let pipe_fds = self
            .cgi_pipes
            .iter()
            .filter_map(|(fd, pipe)| (pipe.job_id() == job_id).then_some(*fd))
            .collect::<Vec<_>>();
        for fd in pipe_fds {
            self.epoll.delete(fd);
            self.cgi_pipes.remove(&fd);
        }

        let Some(client) = self.clients.get_mut(&job.client_fd) else {
            return;
        };
        client.waiting_cgi = None;

        let mut response = match result {
            Ok(response) => response,
            Err(CgiError::Timeout) => error_response(504, &job.server),
            Err(CgiError::Io(error)) => {
                eprintln!("cgi I/O error: {error}");
                error_response(500, &job.server)
            }
            Err(CgiError::InvalidOutput) => error_response(500, &job.server),
        };
        Self::decorate_response(
            &mut response,
            &job.session,
            job.close_after_write,
            job.http10,
        );
        self.queue_response(job.client_fd, response, job.close_after_write);
    }

    fn select_server_index(&self, fd: RawFd, request: &Request) -> Option<usize> {
        let client = self.clients.get(&fd)?;
        let listener = self.listeners.get(&client.listen_fd)?;
        let host = request.host();

        if let Some(host) = host {
            for index in &listener.servers {
                let server = self.config.servers.get(*index)?;
                if server
                    .server_names
                    .iter()
                    .any(|name| host_without_port(name) == host)
                {
                    return Some(*index);
                }
            }
        }

        listener.servers.first().copied()
    }

    fn touch_session(&mut self, request: &Request) -> SessionInfo {
        let requested_id = request
            .header("cookie")
            .and_then(|cookie| find_cookie(cookie, SESSION_COOKIE));

        if let Some(id) = requested_id {
            if let Some(session) = self.sessions.get_mut(&id) {
                session.last_seen = Instant::now();
                session.visits = session.visits.saturating_add(1);
                return SessionInfo {
                    id,
                    visits: session.visits,
                    is_new: false,
                };
            }
        }

        self.session_counter = self.session_counter.saturating_add(1);
        let id = format!(
            "{:x}{:x}{:x}",
            now_millis(),
            std::process::id(),
            self.session_counter
        );
        self.sessions.insert(
            id.clone(),
            Session {
                last_seen: Instant::now(),
                visits: 1,
            },
        );
        SessionInfo {
            id,
            visits: 1,
            is_new: true,
        }
    }

    fn close_timed_out_clients(&mut self) {
        let timeout = self.config.request_timeout;
        let now = Instant::now();
        let timed_out = self
            .clients
            .iter()
            .filter_map(|(fd, client)| {
                if client.waiting_cgi.is_none() && now.duration_since(client.last_active) >= timeout
                {
                    Some(*fd)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        for fd in timed_out {
            let has_response = self
                .clients
                .get(&fd)
                .map(|client| !client.write_buffer.is_empty())
                .unwrap_or(false);
            if has_response {
                if let Some(client) = self.clients.get_mut(&fd) {
                    client.close_after_write = true;
                }
                if let Err(error) = self.epoll.modify(fd, CLIENT_READ_WRITE_EVENTS) {
                    eprintln!("failed to enable write notifications for timed out client: {error}");
                    self.close_client(fd);
                }
            } else {
                let response = self.response_for_bad_request(fd, 408);
                self.queue_response(fd, response, true);
            }
        }
    }

    fn expire_sessions(&mut self) {
        let now = Instant::now();
        let ttl = Duration::from_secs(60 * 60);
        self.sessions
            .retain(|_, session| now.duration_since(session.last_seen) < ttl);
    }

    fn close_client(&mut self, fd: RawFd) {
        if let Some(job_id) = self.clients.get(&fd).and_then(|client| client.waiting_cgi) {
            if let Some(job) = self.cgi_jobs.remove(&job_id) {
                drop(job);
            }
            let pipe_fds = self
                .cgi_pipes
                .iter()
                .filter_map(|(pipe_fd, pipe)| (pipe.job_id() == job_id).then_some(*pipe_fd))
                .collect::<Vec<_>>();
            for pipe_fd in pipe_fds {
                self.epoll.delete(pipe_fd);
                self.cgi_pipes.remove(&pipe_fd);
            }
        }
        self.epoll.delete(fd);
        self.clients.remove(&fd);
    }
}

#[derive(Default)]
struct EndpointConfig {
    has_unnamed_default: bool,
    server_names: HashSet<String>,
    servers: Vec<usize>,
}

fn find_cookie(cookie_header: &str, name: &str) -> Option<String> {
    for part in cookie_header.split(';') {
        if let Some((cookie_name, value)) = part.trim().split_once('=') {
            if cookie_name == name {
                return Some(value.to_string());
            }
        }
    }
    None
}
