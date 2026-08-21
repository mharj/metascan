# greedy - Fast Posix File Metadata Scanner

[![License](https://img.shields.io/badge/License-MIT-green.svg)](#)

A high-performance, parallel directory scanner that generates detailed filesystem metadata for large-scale analysis. Optimized for terabyte-scale datasets and millions of files.

## What It Does

Scans directory trees and produces structured JSON output containing:
- **Total stats**: file count, directory count, total size, last modified/created/accessed timestamps
- **Per-UID breakdown**: files/dirs/size/timestamps owned by each user
- **Per-GID breakdown**: files/dirs/size/timestamps by group
- **Age distribution**: size of data by file age (year-based buckets)
- **Performance metrics**: scan duration and files/sec throughput

Output is JSON-formatted, designed for database ingestion and further analysis.

## Key Features

- **Parallel scanning** with configurable thread count
- **Static binary** (musl-compiled) with zero external dependencies
- **Low memory footprint** via streaming aggregation
- **Configurable excludes** to skip loopback mounts (.snapshot, etc.)
- **Config file support** with CLI overrides
- **Library API** for use in Rust projects

## Build

```bash
cargo build --release
```

Static musl binary: `target/x86_64-unknown-linux-musl/release/greedy`

## Usage

### Basic scan
```bash
sudo ./greedy /path/to/scan
```

### With config file
```bash
sudo ./greedy -c config.json /path/to/scan
```

### Override thread count
```bash
sudo ./greedy -threads 32 /path/to/scan
sudo ./greedy -c config.json -threads 8 /path/to/scan
```

### Config file (config.json)
```json
{
  "threads": 16,
  "excludes": [".snapshot"]
}
```

CLI args override config values.

## Library Usage

```rust
use greedy::models::Config;
use greedy::scanner::scan_directory;

let config = Config::from_file("config.json")?;
let output = scan_directory("/project_xyz", Some(&config))?;
println!("Files: {}", output.all.files);
```

## Output Example

```json
{
  "all": {
    "files": 714,
    "dirs": 83,
    "size": 1347140,
    "mtime": 1787240798,
    "ctime": 1787240798,
    "atime": 1787240799
  },
  "uids": [...],
  "gids": [...],
  "year": {
    "0": 969625,
    "1": 352926
  },
  "stats": {
    "elapsed": 10,
    "fps": 132833
  }
}
```

