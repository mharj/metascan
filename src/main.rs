mod cli;
mod models;
mod scanner;

#[cfg(all(target_os = "linux", target_env = "musl"))]
use mimalloc::MiMalloc;

#[cfg(all(target_os = "linux", target_env = "musl"))]
#[global_allocator]
static GLOBAL_ALLOCATOR: MiMalloc = MiMalloc;

use crate::cli::parse_args;
use crate::models::Config;
use crate::scanner::{default_thread_count, scan_directory};

fn main() {
    let default_threads: u32 = default_thread_count();
    let args: Vec<String> = std::env::args().collect();

    let (target, threads, config_path) = match parse_args(&args, default_threads) {
        Ok(v) => v,
        Err(code) => std::process::exit(code),
    };

    // Try to load config
    let config = if let Some(ref path) = config_path {
        // Load from specified config file
        match Config::from_file(path) {
            Ok(mut cfg) => {
                // CLI -threads arg overrides config file if provided
                if threads != default_threads {
                    cfg.threads = Some(threads);
                }
                Some(cfg)
            }
            Err(e) => {
                eprintln!("Error loading config file '{}': {}", path, e);
                std::process::exit(255);
            }
        }
    } else {
        // Create a config from CLI args
        Some(Config {
            threads: Some(threads),
            excludes: None,
        })
    };

    let out = match scan_directory(&target, config.as_ref()) {
        Ok(output) => output,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(254);
        }
    };

    match serde_json::to_string(&out) {
        Ok(s) => {
            println!("{}", s);
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("serialization error: {}", e);
            std::process::exit(1);
        }
    }
}
