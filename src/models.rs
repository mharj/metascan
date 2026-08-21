use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::os::unix::fs::MetadataExt;

/// Scan configuration
///
/// Configures number of threads and directories to exclude from scanning.
/// Excludes are matched by directory name (filename-based).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// Number of worker threads (defaults to 16)
    #[serde(default)]
    pub threads: Option<u32>,
    /// Directory names to exclude (exact filename match)
    #[serde(default)]
    pub excludes: Option<Vec<String>>,
}

impl Config {
    /// Load config from a JSON file
    pub fn from_file(path: &str) -> Result<Self, String> {
        let content =
            fs::read_to_string(path).map_err(|e| format!("Failed to read config file: {}", e))?;
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse config file: {}", e))
    }
}

/// Aggregated filesystem statistics
///
/// Contains file/directory counts, total size, and timestamp ranges.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct StatAgg {
    /// Number of files
    pub files: u64,
    /// Number of directories
    pub dirs: u64,
    /// Total size in bytes
    pub size: u64,
    /// Latest modification time (Unix timestamp)
    pub mtime: i64,
    /// Latest change time (Unix timestamp)
    pub ctime: i64,
    /// Latest access time (Unix timestamp)
    pub atime: i64,
}

impl StatAgg {
    pub fn add_meta(&mut self, st: &fs::Metadata, now: i64) {
        let mtime = st.mtime();
        let ctime = st.ctime();
        let atime = st.atime();

        if mtime > self.mtime && mtime < now {
            self.mtime = mtime;
        }
        if ctime > self.ctime && ctime < now {
            self.ctime = ctime;
        }
        if atime > self.atime && atime < now {
            self.atime = atime;
        }

        self.size = self.size.saturating_add(st.size());
        if st.file_type().is_dir() {
            self.dirs = self.dirs.saturating_add(1);
        } else {
            self.files = self.files.saturating_add(1);
        }
    }

    pub fn merge(&mut self, sub: &StatAgg) {
        self.files = self.files.saturating_add(sub.files);
        self.dirs = self.dirs.saturating_add(sub.dirs);
        self.size = self.size.saturating_add(sub.size);
        self.mtime = self.mtime.max(sub.mtime);
        self.ctime = self.ctime.max(sub.ctime);
        self.atime = self.atime.max(sub.atime);
    }
}

#[derive(Debug, Serialize)]
pub struct UidEntry {
    pub uid: u32,
    pub files: u64,
    pub dirs: u64,
    pub size: u64,
    pub mtime: i64,
    pub ctime: i64,
    pub atime: i64,
}

#[derive(Debug, Serialize)]
pub struct GidEntry {
    pub gid: u32,
    pub files: u64,
    pub dirs: u64,
    pub size: u64,
    pub mtime: i64,
    pub ctime: i64,
    pub atime: i64,
}

#[derive(Debug, Default, Serialize)]
pub struct Stats {
    pub elapsed: u128,
    pub fps: u64,
}

/// Scan results in JSON-serializable format
///
/// Contains comprehensive metadata from a directory scan.
#[derive(Debug, Serialize)]
pub struct Output {
    /// Total aggregated statistics
    pub all: StatAgg,
    /// Statistics per user ID
    pub uids: Vec<UidEntry>,
    /// Statistics per group ID
    pub gids: Vec<GidEntry>,
    /// Size distribution by file age (year buckets)
    pub year: BTreeMap<i64, u64>,
    /// Scan performance metrics
    pub stats: Stats,
    /// Error message if permissions were denied
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub all: StatAgg,
    pub per_uid: HashMap<u32, StatAgg>,
    pub per_gid: HashMap<u32, StatAgg>,
    pub year: BTreeMap<i64, u64>,
    pub permission_ok: bool,
    pub stats: Stats,
    pub error: Option<String>,
}

impl ScanResult {
    pub fn merge(&mut self, other: &ScanResult) {
        self.all.merge(&other.all);

        for (uid, stat) in &other.per_uid {
            self.per_uid.entry(*uid).or_default().merge(stat);
        }
        for (gid, stat) in &other.per_gid {
            self.per_gid.entry(*gid).or_default().merge(stat);
        }
        for (year, size) in &other.year {
            let entry = self.year.entry(*year).or_insert(0);
            *entry = entry.saturating_add(*size);
        }

        self.permission_ok = self.permission_ok && other.permission_ok;
    }

    pub fn to_uid_entries(&self) -> Vec<UidEntry> {
        let mut entries: Vec<UidEntry> = self
            .per_uid
            .iter()
            .map(|(uid, s)| UidEntry {
                uid: *uid,
                files: s.files,
                dirs: s.dirs,
                size: s.size,
                mtime: s.mtime,
                ctime: s.ctime,
                atime: s.atime,
            })
            .collect();
        entries.sort_by_key(|x| x.uid);
        entries
    }

    pub fn to_gid_entries(&self) -> Vec<GidEntry> {
        let mut entries: Vec<GidEntry> = self
            .per_gid
            .iter()
            .map(|(gid, s)| GidEntry {
                gid: *gid,
                files: s.files,
                dirs: s.dirs,
                size: s.size,
                mtime: s.mtime,
                ctime: s.ctime,
                atime: s.atime,
            })
            .collect();
        entries.sort_by_key(|x| x.gid);
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::StatAgg;

    #[test]
    fn merge_accumulates_and_uses_max_times() {
        let mut a = StatAgg {
            files: 2,
            dirs: 1,
            size: 10,
            mtime: 100,
            ctime: 90,
            atime: 80,
        };
        let b = StatAgg {
            files: 3,
            dirs: 4,
            size: 20,
            mtime: 110,
            ctime: 85,
            atime: 120,
        };

        a.merge(&b);

        assert_eq!(a.files, 5);
        assert_eq!(a.dirs, 5);
        assert_eq!(a.size, 30);
        assert_eq!(a.mtime, 110);
        assert_eq!(a.ctime, 90);
        assert_eq!(a.atime, 120);
    }
}
