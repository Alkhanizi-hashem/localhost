use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use crate::http::Method;

#[derive(Clone, Debug)]
pub struct Config {
    pub servers: Vec<ServerConfig>,
    pub request_timeout: Duration,
    pub cgi_timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub host: String,
    pub ports: Vec<u16>,
    pub server_names: Vec<String>,
    pub error_pages: HashMap<u16, PathBuf>,
    pub client_max_body_size: usize,
    pub routes: Vec<RouteConfig>,
}

#[derive(Clone, Debug)]
pub struct RouteConfig {
    pub path: String,
    pub methods: HashSet<Method>,
    pub root: Option<PathBuf>,
    pub redirect: Option<RedirectConfig>,
    pub index: Option<String>,
    pub directory_listing: bool,
    pub upload_store: Option<PathBuf>,
    pub cgi: HashMap<String, PathBuf>,
}

#[derive(Clone, Debug)]
pub struct RedirectConfig {
    pub status: u16,
    pub location: String,
}

impl Config {
    pub fn load(path: &str) -> Result<(Self, Vec<String>), String> {
        let content =
            fs::read_to_string(path).map_err(|error| format!("failed to read {path}: {error}"))?;
        let tokens = tokenize(&content);
        let mut parser = Parser::new(tokens);
        let mut config = parser.parse_config()?;
        let warnings = config.validate();
        Ok((config, warnings))
    }

    fn validate(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        self.servers.retain_mut(|server| {
            if server.host.is_empty() {
                warnings.push("discarded a server with an empty host".to_string());
                return false;
            }
            if server.ports.is_empty() {
                warnings.push(format!(
                    "discarded server {} because it has no ports",
                    server.host
                ));
                return false;
            }

            let mut seen_ports = HashSet::new();
            if let Some(duplicate_port) = server
                .ports
                .iter()
                .copied()
                .find(|port| !seen_ports.insert(*port))
            {
                warnings.push(format!(
                    "discarded server {} because port {duplicate_port} is configured more than once",
                    server.host
                ));
                return false;
            }

            server.server_names = server
                .server_names
                .iter()
                .map(|name| name.to_ascii_lowercase())
                .collect();

            if server.routes.is_empty() {
                warnings.push(format!(
                    "server {} has no routes; added a default route rooted at ./www",
                    server.host
                ));
                server.routes.push(RouteConfig::default_root());
            }

            server.routes.retain(|route| {
                if !route.path.starts_with('/') {
                    warnings.push(format!(
                        "discarded route {} because it does not start with /",
                        route.path
                    ));
                    return false;
                }
                true
            });
            server
                .routes
                .sort_by(|left, right| right.path.len().cmp(&left.path.len()));

            !server.ports.is_empty() && !server.routes.is_empty()
        });

        warnings
    }
}

impl ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            ports: Vec::new(),
            server_names: Vec::new(),
            error_pages: HashMap::new(),
            client_max_body_size: 1_048_576,
            routes: Vec::new(),
        }
    }
}

impl RouteConfig {
    fn new(path: String) -> Self {
        Self {
            path,
            methods: HashSet::new(),
            root: None,
            redirect: None,
            index: None,
            directory_listing: false,
            upload_store: None,
            cgi: HashMap::new(),
        }
    }

    fn default_root() -> Self {
        let mut methods = HashSet::new();
        methods.insert(Method::Get);
        Self {
            path: "/".to_string(),
            methods,
            root: Some(PathBuf::from("www")),
            redirect: None,
            index: Some("index.html".to_string()),
            directory_listing: false,
            upload_store: None,
            cgi: HashMap::new(),
        }
    }

    pub fn allowed_methods_header(&self) -> String {
        if self.methods.is_empty() {
            return "GET, POST, DELETE".to_string();
        }

        let mut methods: Vec<String> = self.methods.iter().map(ToString::to_string).collect();
        methods.sort();
        methods.join(", ")
    }
}

struct Parser {
    tokens: Vec<String>,
    position: usize,
}

impl Parser {
    fn new(tokens: Vec<String>) -> Self {
        Self {
            tokens,
            position: 0,
        }
    }

    fn parse_config(&mut self) -> Result<Config, String> {
        let mut config = Config {
            servers: Vec::new(),
            request_timeout: Duration::from_secs(30),
            cgi_timeout: Duration::from_secs(5),
        };

        while !self.is_done() {
            let token = self.next_required("directive")?;
            match token.as_str() {
                "server" => config.servers.push(self.parse_server()?),
                "request_timeout" => {
                    let seconds = self.parse_u64("request_timeout")?;
                    self.expect(";")?;
                    config.request_timeout = Duration::from_secs(seconds);
                }
                "cgi_timeout" => {
                    let seconds = self.parse_u64("cgi_timeout")?;
                    self.expect(";")?;
                    config.cgi_timeout = Duration::from_secs(seconds);
                }
                other => return Err(format!("unknown top-level directive {other}")),
            }
        }

        if config.servers.is_empty() {
            return Err("configuration must contain at least one server block".to_string());
        }

        Ok(config)
    }

    fn parse_server(&mut self) -> Result<ServerConfig, String> {
        self.expect("{")?;
        let mut server = ServerConfig::default();

        while !self.consume("}") {
            let directive = self.next_required("server directive")?;
            match directive.as_str() {
                "host" => {
                    server.host = self.next_required("host")?;
                    self.expect(";")?;
                }
                "port" | "ports" | "listen" => {
                    while !self.consume(";") {
                        let port = self.parse_current_u16("port")?;
                        server.ports.push(port);
                    }
                }
                "server_name" => {
                    while !self.consume(";") {
                        server.server_names.push(self.next_required("server name")?);
                    }
                }
                "error_page" => {
                    let code = self.parse_u16("status code")?;
                    let path = self.next_required("error page path")?;
                    self.expect(";")?;
                    server.error_pages.insert(code, PathBuf::from(path));
                }
                "client_max_body_size" => {
                    let size = self.next_required("body size")?;
                    self.expect(";")?;
                    server.client_max_body_size = parse_size(&size)?;
                }
                "route" => {
                    let path = self.next_required("route path")?;
                    server.routes.push(self.parse_route(path)?);
                }
                other => return Err(format!("unknown server directive {other}")),
            }
        }

        Ok(server)
    }

    fn parse_route(&mut self, path: String) -> Result<RouteConfig, String> {
        self.expect("{")?;
        let mut route = RouteConfig::new(path);

        while !self.consume("}") {
            let directive = self.next_required("route directive")?;
            match directive.as_str() {
                "methods" => {
                    while !self.consume(";") {
                        let method = self.next_required("method")?;
                        route.methods.insert(Method::from_token(&method));
                    }
                }
                "root" => {
                    route.root = Some(PathBuf::from(self.next_required("root")?));
                    self.expect(";")?;
                }
                "redirect" => {
                    let first = self.next_required("redirect status or location")?;
                    let (status, location) = if let Ok(status) = first.parse::<u16>() {
                        (status, self.next_required("redirect location")?)
                    } else {
                        (302, first)
                    };
                    self.expect(";")?;
                    route.redirect = Some(RedirectConfig { status, location });
                }
                "index" | "default_file" => {
                    route.index = Some(self.next_required("index")?);
                    self.expect(";")?;
                }
                "directory_listing" | "autoindex" => {
                    let value = self.next_required("on or off")?;
                    self.expect(";")?;
                    route.directory_listing = parse_on_off(&value)?;
                }
                "upload_store" => {
                    route.upload_store = Some(PathBuf::from(self.next_required("upload store")?));
                    self.expect(";")?;
                }
                "cgi" => {
                    let extension = self.next_required("cgi extension")?;
                    let interpreter = self.next_required("cgi interpreter")?;
                    self.expect(";")?;
                    route.cgi.insert(extension, PathBuf::from(interpreter));
                }
                other => return Err(format!("unknown route directive {other}")),
            }
        }

        Ok(route)
    }

    fn parse_u16(&mut self, label: &str) -> Result<u16, String> {
        let token = self.next_required(label)?;
        token
            .parse::<u16>()
            .map_err(|_| format!("invalid {label}: {token}"))
    }

    fn parse_u64(&mut self, label: &str) -> Result<u64, String> {
        let token = self.next_required(label)?;
        token
            .parse::<u64>()
            .map_err(|_| format!("invalid {label}: {token}"))
    }

    fn parse_current_u16(&mut self, label: &str) -> Result<u16, String> {
        let token = self.next_required(label)?;
        token
            .parse::<u16>()
            .map_err(|_| format!("invalid {label}: {token}"))
    }

    fn expect(&mut self, expected: &str) -> Result<(), String> {
        let token = self.next_required(expected)?;
        if token == expected {
            Ok(())
        } else {
            Err(format!("expected {expected}, found {token}"))
        }
    }

    fn consume(&mut self, expected: &str) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn next_required(&mut self, label: &str) -> Result<String, String> {
        if self.position >= self.tokens.len() {
            return Err(format!("expected {label}, reached end of file"));
        }
        let token = self.tokens[self.position].clone();
        self.position += 1;
        Ok(token)
    }

    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.position).map(String::as_str)
    }

    fn is_done(&self) -> bool {
        self.position >= self.tokens.len()
    }
}

fn tokenize(content: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = content.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '#' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                while let Some(next) = chars.peek() {
                    if *next == '\n' {
                        break;
                    }
                    chars.next();
                }
            }
            '{' | '}' | ';' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                tokens.push(ch.to_string());
            }
            ch if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn parse_size(value: &str) -> Result<usize, String> {
    let lower = value.to_ascii_lowercase();
    let (digits, multiplier) = if let Some(stripped) = lower.strip_suffix('k') {
        (stripped, 1024usize)
    } else if let Some(stripped) = lower.strip_suffix('m') {
        (stripped, 1024usize * 1024)
    } else if let Some(stripped) = lower.strip_suffix('g') {
        (stripped, 1024usize * 1024 * 1024)
    } else {
        (lower.as_str(), 1usize)
    };

    let number = digits
        .parse::<usize>()
        .map_err(|_| format!("invalid size: {value}"))?;
    number
        .checked_mul(multiplier)
        .ok_or_else(|| format!("size is too large: {value}"))
}

fn parse_on_off(value: &str) -> Result<bool, String> {
    match value {
        "on" | "true" | "1" => Ok(true),
        "off" | "false" | "0" => Ok(false),
        other => Err(format!("expected on or off, found {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_size, tokenize};

    #[test]
    fn tokenizes_blocks_and_semicolons() {
        let tokens = tokenize("server { host 127.0.0.1; # comment\n }");
        assert_eq!(tokens, vec!["server", "{", "host", "127.0.0.1", ";", "}"]);
    }

    #[test]
    fn parses_body_size_suffixes() {
        assert_eq!(parse_size("1k").unwrap(), 1024);
        assert_eq!(parse_size("2M").unwrap(), 2 * 1024 * 1024);
        assert_eq!(parse_size("12").unwrap(), 12);
    }

    #[test]
    fn rejects_server_with_duplicate_port() {
        let mut config = super::Config {
            servers: vec![super::ServerConfig {
                host: "127.0.0.1".to_string(),
                ports: vec![8080, 8080],
                server_names: Vec::new(),
                error_pages: std::collections::HashMap::new(),
                client_max_body_size: 1024,
                routes: vec![super::RouteConfig::default_root()],
            }],
            request_timeout: std::time::Duration::from_secs(30),
            cgi_timeout: std::time::Duration::from_secs(5),
        };

        let warnings = config.validate();
        assert!(config.servers.is_empty());
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("configured more than once")));
    }
}
