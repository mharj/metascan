//! Fast parallel filesystem metadata scanner.
//!
//! Generates detailed JSON output containing file counts, sizes, ownership, and age distribution
//! from large directory trees. Optimized for terabyte-scale datasets.
//!
//! # Example
//!
//! ```no_run
//! use greedy::models::Config;
//! use greedy::scanner::scan_directory;
//!
//! let config = Config::from_file("config.json")?;
//! let output = scan_directory("/path/to/scan", Some(&config))?;
//! println!("Total files: {}", output.all.files);
//! println!("Total size: {}", output.all.size);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod models;
pub mod scanner;
