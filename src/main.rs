mod cgi;
mod config;
mod epoll;
mod ffi;
mod http;
mod router;
mod server;
mod util;

use std::env;
use std::process;

use config::Config;
use server::Server;

fn main() {
    let config_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "config/default.conf".to_string());

    let (config, warnings) = match Config::load(&config_path) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("configuration error: {error}");
            process::exit(1);
        }
    };

    for warning in warnings {
        eprintln!("configuration warning: {warning}");
    }

    let mut server = match Server::new(config) {
        Ok((server, warnings)) => {
            for warning in warnings {
                eprintln!("server warning: {warning}");
            }
            server
        }
        Err(error) => {
            eprintln!("server error: {error}");
            process::exit(1);
        }
    };

    if let Err(error) = server.run() {
        eprintln!("server stopped: {error}");
        process::exit(1);
    }
}
