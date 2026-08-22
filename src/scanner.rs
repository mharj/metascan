use crate::models::{Config, Output, ScanResult, Stats};
use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct QueueState {
    queue: VecDeque<PathBuf>,
    active: usize,
    closed: bool,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn scan_one_dir(
    dir: &PathBuf,
    now: i64,
    excludes: &Option<Vec<String>>,
) -> (ScanResult, Vec<PathBuf>) {
    let mut local = ScanResult {
        permission_ok: true,
        ..ScanResult::default()
    };
    let mut discovered = Vec::new();
    // Attempt to read the directory entries; if it fails, mark permission as not okay and return
    let entries = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => {
            local.permission_ok = false;
            return (local, discovered);
        }
    };
    // Iterate over the directory entries
    for entry_res in entries {
        let entry = match entry_res {
            Ok(e) => e,
            Err(_) => {
                local.permission_ok = false;
                continue;
            }
        };

        // Skip files matching exclude patterns (filename-based)
        if let Some(filename_str) = entry.file_name().to_str() {
            if let Some(exclude_list) = excludes {
                if exclude_list.iter().any(|pattern| filename_str == pattern) {
                    continue;
                }
            }
        }

        let path = entry.path();

        let st = match fs::symlink_metadata(&path) {
            Ok(s) => s,
            Err(_) => {
                local.permission_ok = false;
                continue;
            }
        };

        local.all.add_meta(&st, now);

        let uid = st.uid();
        let gid = st.gid();
        local.per_uid.entry(uid).or_default().add_meta(&st, now);
        local.per_gid.entry(gid).or_default().add_meta(&st, now);

        let mtime = st.mtime();
        let delta = (now - mtime) as f64;
        let seconds_in_year = 31_536_000.0;
        let bucket = (delta / seconds_in_year).floor() as i64;
        let year_entry = local.year.entry(bucket).or_insert(0);
        *year_entry = year_entry.saturating_add(st.size());

        if st.file_type().is_dir() {
            discovered.push(path);
        }
    }

    (local, discovered)
}

pub fn scan_parallel(
    root: PathBuf,
    thread_count: u32,
    excludes: Option<Vec<String>>,
) -> ScanResult {
    let workers = thread_count.max(1) as usize;
    let excludes = Arc::new(excludes);

    // Create shared thread-safe state: work queue protected by mutex and condition variable for worker coordination
    let shared = Arc::new((
        Mutex::new(QueueState {
            queue: {
                let mut q = VecDeque::new();
                q.push_back(root);
                q
            },
            active: 0,
            closed: false,
        }),
        Condvar::new(),
    ));

    let mut handles = Vec::with_capacity(workers);

    for _ in 0..workers {
        // Clone the Arc to share ownership of the queue state with each worker thread
        let shared_state = Arc::clone(&shared);
        let excludes_clone = Arc::clone(&excludes);
        // Spawn a new worker thread
        handles.push(thread::spawn(move || {
            // Each worker thread maintains its own local ScanResult accumulator
            let mut local_acc = ScanResult {
                permission_ok: true,
                ..ScanResult::default()
            };
            // Worker thread loop: continuously process directories from the queue until it's closed
            loop {
                let dir = {
                    let (lock, cvar) = &*shared_state;
                    let mut state = lock.lock().expect("queue lock poisoned");

                    loop {
                        if let Some(d) = state.queue.pop_front() {
                            state.active += 1;
                            break d;
                        }

                        if state.closed {
                            return local_acc;
                        }

                        state = cvar.wait(state).expect("queue wait poisoned");
                    }
                };

                let (scan_result, discovered) = scan_one_dir(&dir, now_unix(), &*excludes_clone);
                local_acc.merge(&scan_result);

                let (lock, cvar) = &*shared_state;
                let mut state = lock.lock().expect("queue lock poisoned");

                for d in discovered {
                    state.queue.push_back(d);
                }

                state.active = state.active.saturating_sub(1);

                if state.queue.is_empty() && state.active == 0 {
                    state.closed = true;
                    cvar.notify_all();
                } else if !state.queue.is_empty() {
                    cvar.notify_all();
                }
            }
        }));
    }
    // Wait for all worker threads to finish and collect their results
    let mut merged = ScanResult {
        permission_ok: true,
        ..ScanResult::default()
    };
    merged.all.dirs = 1; // include base dir

    for h in handles {
        if let Ok(worker_out) = h.join() {
            merged.merge(&worker_out);
        } else {
            merged.permission_ok = false;
        }
    }

    merged
}

/// Scan a directory tree and return aggregated metadata
///
/// # Arguments
///
/// * `path` - Root directory to scan
/// * `config` - Optional configuration (threads, excludes). Defaults to 16 threads if None.
///
/// # Example
///
/// ```no_run
/// use metascan::scanner::scan_directory;
/// use metascan::models::Config;
///
/// let output = scan_directory("/project_xyz", None)?;
/// println!("Files: {}", output.all.files);
/// # Ok::<(), String>(())
/// ```
pub fn scan_directory<P: AsRef<Path>>(path: P, config: Option<&Config>) -> Result<Output, String> {
    let target = PathBuf::from(path.as_ref());

    if !target.exists() {
        return Err(format!("Directory does not exist: {}", target.display()));
    }

    if fs::read_dir(&target).is_err() {
        return Err(format!("Permission denied: {}", target.display()));
    }

    let threads = config.and_then(|c| c.threads).unwrap_or(16);
    let excludes = config.and_then(|c| c.excludes.clone());

    let start = SystemTime::now();
    let mut result = scan_parallel(target.clone(), threads, excludes);
    let elapsed = start.elapsed().map(|d| d.as_millis()).unwrap_or(0);
    let elapsed_sec = (elapsed as f64) / 1000.0;
    let fps = if elapsed_sec > 0.0 {
        ((result.all.files + result.all.dirs) as f64 / elapsed_sec).round() as u64
    } else {
        0
    };

    result.stats = Stats { elapsed, fps };
    result.error = if result.permission_ok {
        None
    } else {
        Some(format!(
            "{} have some permission problems, check root access",
            target.display()
        ))
    };

    Ok(Output {
        all: result.all,
        uids: result.to_uid_entries(),
        gids: result.to_gid_entries(),
        year: result.year,
        stats: result.stats,
        error: result.error,
    })
}
