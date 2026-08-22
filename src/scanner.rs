use crate::models::{Config, Output, ScanResult, Stats};
use crossbeam_deque::{Injector, Steal, Stealer, Worker};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn default_thread_count() -> u32 {
    // Clamp default workers to avoid oversubscription and lock/allocator contention.
    // On NFS-heavy workloads, 16 workers is often a practical saturation point.
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(16)
        .clamp(1, 16)
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn year_bucket(mtime: i64, now: i64) -> i64 {
    let delta = (now - mtime) as f64;
    let seconds_in_year = 31_536_000.0;
    (delta / seconds_in_year).floor() as i64
}

fn scan_one_dir(
    dir: &PathBuf,
    now: i64,
    excludes: &Option<HashSet<OsString>>,
) -> (ScanResult, Vec<PathBuf>) {
    let mut local = ScanResult {
        permission_ok: true,
        ..ScanResult::default()
    };
    let mut discovered = Vec::with_capacity(32);
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

        if let Some(exclude_set) = excludes.as_ref() {
            let entry_name = entry.file_name();
            if exclude_set.contains(&entry_name) {
                continue;
            }
        }

        let entry_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => {
                local.permission_ok = false;
                continue;
            }
        };

        // For non-symlinks, use entry.metadata() to avoid path reconstruction.
        let st = if entry_type.is_symlink() {
            let path = entry.path();
            match fs::symlink_metadata(&path) {
                Ok(s) => s,
                Err(_) => {
                    local.permission_ok = false;
                    continue;
                }
            }
        } else {
            match entry.metadata() {
                Ok(s) => s,
                Err(_) => {
                    local.permission_ok = false;
                    continue;
                }
            }
        };

        local.all.add_meta(&st, now);

        let uid = st.uid();
        let gid = st.gid();
        local.per_uid.entry(uid).or_default().add_meta(&st, now);
        local.per_gid.entry(gid).or_default().add_meta(&st, now);

        let mtime = st.mtime();
        let bucket = year_bucket(mtime, now);
        let year_entry = local.year.entry(bucket).or_insert(0);
        *year_entry = year_entry.saturating_add(st.size());

        if entry_type.is_dir() {
            discovered.push(entry.path());
        }
    }

    (local, discovered)
}

pub fn scan_parallel(
    root: PathBuf,
    thread_count: u32,
    excludes: Option<Vec<String>>,
) -> ScanResult {
    const LOCAL_BATCH_MAX: usize = 32;
    const STEAL_INTERVAL: usize = 8;
    const EMPTY_SPIN_LIMIT: usize = 64;

    let workers = thread_count.max(1) as usize;
    let scan_now = now_unix();
    let excludes = Arc::new(excludes.map(|list| {
        list.into_iter()
            .map(OsString::from)
            .collect::<HashSet<_>>()
    }));

    let injector = Arc::new(Injector::new());
    injector.push(root);
    let outstanding = Arc::new(AtomicUsize::new(1));

    let local_queues: Vec<Worker<PathBuf>> =
        (0..workers).map(|_| Worker::new_fifo()).collect();
    let stealers: Vec<Stealer<PathBuf>> = local_queues.iter().map(Worker::stealer).collect();

    let mut handles = Vec::with_capacity(workers);

    for (worker_index, local) in local_queues.into_iter().enumerate() {
        let injector_clone = Arc::clone(&injector);
        let outstanding_clone = Arc::clone(&outstanding);
        let excludes_clone = Arc::clone(&excludes);
        let stealers_clone = stealers.clone();
        let scan_now_copy = scan_now;

        handles.push(thread::spawn(move || {
            let mut local_acc = ScanResult {
                permission_ok: true,
                ..ScanResult::default()
            };
            let mut empty_streak = 0usize;
            let mut batch = Vec::with_capacity(LOCAL_BATCH_MAX);

            loop {
                let dir = if let Some(d) = local.pop() {
                    Some(d)
                } else {
                    let mut found = match injector_clone.steal_batch_and_pop(&local) {
                        Steal::Success(d) => Some(d),
                        Steal::Retry | Steal::Empty => None,
                    };

                    if found.is_none() && empty_streak % STEAL_INTERVAL == 0 {
                        let victim_offset = worker_index.wrapping_add(empty_streak);
                        for idx in 0..stealers_clone.len() {
                            let victim = (victim_offset + idx) % stealers_clone.len();
                            if victim == worker_index {
                                continue;
                            }
                            match stealers_clone[victim].steal_batch_and_pop(&local) {
                                Steal::Success(d) => {
                                    found = Some(d);
                                    break;
                                }
                                Steal::Retry | Steal::Empty => continue,
                            }
                        }
                    }

                    found
                };

                let Some(dir) = dir else {
                    if outstanding_clone.load(Ordering::Acquire) == 0 {
                        break;
                    }

                    empty_streak = empty_streak.saturating_add(1);
                    if empty_streak <= EMPTY_SPIN_LIMIT {
                        std::hint::spin_loop();
                    } else {
                        thread::yield_now();
                    }
                    continue;
                };

                empty_streak = 0;
                batch.clear();
                batch.push(dir);

                while batch.len() < LOCAL_BATCH_MAX {
                    match local.pop() {
                        Some(next) => batch.push(next),
                        None => break,
                    }
                }

                for batch_dir in batch.drain(..) {
                    let (scan_result, discovered) =
                        scan_one_dir(&batch_dir, scan_now_copy, &*excludes_clone);
                    local_acc.merge(&scan_result);

                    outstanding_clone.fetch_add(discovered.len(), Ordering::AcqRel);
                    for d in discovered {
                        local.push(d);
                    }

                    if outstanding_clone.fetch_sub(1, Ordering::AcqRel) == 1 {
                        return local_acc;
                    }
                }
            }

            local_acc
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

    let threads = config
        .and_then(|c| c.threads)
        .unwrap_or_else(default_thread_count);
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
