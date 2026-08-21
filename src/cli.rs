use std::path::PathBuf;

fn usage(bin: &str, default_threads: u32) {
    println!(
        "Build: {} {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    );
    println!("Usage: {} [options] /path/to/mount_point", bin);
    println!("Options:");
    println!("\t-threads {}", default_threads);
    println!("\t-c <config.json>\tConfig file path");
}

pub fn parse_args(
    args: &[String],
    default_threads: u32,
) -> Result<(PathBuf, u32, Option<String>), i32> {
    if args.len() < 2 {
        usage(&args[0], default_threads);
        return Err(255);
    }

    let mut thread_count = default_threads;
    let mut config_path: Option<String> = None;
    let target = PathBuf::from(args.last().cloned().unwrap_or_default());

    let mut i = 1usize;
    while i + 1 < args.len() {
        if args[i] == "-threads" {
            if let Ok(v) = args[i + 1].parse::<u32>() {
                thread_count = v.max(1);
            }
            i += 2;
            continue;
        }
        if args[i] == "-c" {
            config_path = Some(args[i + 1].clone());
            i += 2;
            continue;
        }
        i += 1;
    }

    Ok((target, thread_count, config_path))
}
