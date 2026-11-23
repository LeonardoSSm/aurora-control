//! Read-only access to files written by the providers' own clients.
//! Files are opened with the default Windows share mode (read, write and
//! delete), so the providers can keep writing and rotating them.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

pub fn home_dir() -> Option<PathBuf> {
    env_dir("USERPROFILE").or_else(|| env_dir("HOME"))
}

pub fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadError {
    Missing,
    /// Larger than the limit for its kind: never read in full.
    TooLarge,
    /// Exists but cannot be read, or is not UTF-8.
    Unreadable,
}

/// Reads a whole small file (credentials, profiles, configuration) of at
/// most `max` bytes. Larger files are rejected without being loaded.
pub fn read_limited(path: &Path, max: u64) -> Result<String, ReadError> {
    let file = File::open(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => ReadError::Missing,
        _ => ReadError::Unreadable,
    })?;
    let length = file.metadata().map_err(|_| ReadError::Unreadable)?.len();
    if length > max {
        return Err(ReadError::TooLarge);
    }
    let mut bytes = Vec::new();
    // The file may grow between the size check and the read.
    file.take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| ReadError::Unreadable)?;
    if bytes.len() as u64 > max {
        return Err(ReadError::TooLarge);
    }
    String::from_utf8(bytes).map_err(|_| ReadError::Unreadable)
}

/// Cumulative limits for one scan of local logs: directory entries, files,
/// bytes, parsed records and wall time (audit I06). Once a limit is reached
/// the scan stops and its result is marked partial.
#[derive(Debug)]
pub struct Budget {
    entries: usize,
    files: usize,
    bytes: u64,
    records: usize,
    deadline: Instant,
    reached: bool,
}

impl Budget {
    pub fn new(entries: usize, files: usize, bytes: u64, records: usize, time: Duration) -> Self {
        Self {
            entries,
            files,
            bytes,
            records,
            deadline: Instant::now() + time,
            reached: false,
        }
    }

    /// Codex rollout fallback: a handful of recent files.
    pub fn rollouts() -> Self {
        Self::new(8_192, 6, 4 * 1024 * 1024, 0, Duration::from_millis(1_500))
    }

    /// Claude Code transcripts of the last hours.
    pub fn transcripts() -> Self {
        Self::new(
            20_000,
            256,
            64 * 1024 * 1024,
            200_000,
            Duration::from_millis(2_000),
        )
    }

    fn spend(&mut self, counter: fn(&mut Self) -> &mut usize) -> bool {
        if self.reached || Instant::now() >= self.deadline {
            self.reached = true;
            return false;
        }
        let left = counter(self);
        if *left == 0 {
            self.reached = true;
            return false;
        }
        *left -= 1;
        true
    }

    /// Accounts for one directory entry examined.
    pub fn entry(&mut self) -> bool {
        self.spend(|budget| &mut budget.entries)
    }

    /// Accounts for one file opened.
    pub fn file(&mut self) -> bool {
        self.spend(|budget| &mut budget.files)
    }

    /// Accounts for one parsed record.
    pub fn record(&mut self) -> bool {
        self.spend(|budget| &mut budget.records)
    }

    /// Reserves up to `want` bytes and returns how many may be read.
    fn bytes(&mut self, want: u64) -> u64 {
        let granted = want.min(self.bytes);
        self.bytes -= granted;
        if granted < want {
            self.reached = true;
        }
        granted
    }

    pub fn exhausted(&self) -> bool {
        self.reached
    }
}

/// [`read_tail`] within a budget: counts one file and the bytes read.
pub fn read_tail_within(path: &Path, max: u64, budget: &mut Budget) -> io::Result<Tail> {
    if !budget.file() {
        return Err(io::Error::other("read budget exhausted"));
    }
    let length = std::fs::metadata(path)?.len();
    let granted = budget.bytes(length.min(max));
    if granted == 0 && length > 0 {
        return Err(io::Error::other("read budget exhausted"));
    }
    Ok(Tail {
        text: read_tail(path, granted)?,
        cut: granted < length,
    })
}

/// The end of a file, and whether its beginning was left out (by the
/// per-file limit or by the cumulative budget).
#[derive(Debug)]
pub struct Tail {
    pub text: String,
    pub cut: bool,
}

/// Reads at most `max` bytes from the end of a file. When the read starts in
/// the middle of the file, the first partial line is dropped.
pub fn read_tail(path: &Path, max: u64) -> io::Result<String> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(max);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(usize::try_from(length - start).unwrap_or(0).min(1 << 20));
    file.take(max).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(if start > 0 {
        text.split_once('\n')
            .map(|(_, rest)| rest.to_owned())
            .unwrap_or_default()
    } else {
        text.into_owned()
    })
}

/// Decodes the claims of a JWT without verifying it. Used only for display
/// labels and expiry hints; the provider remains the authority.
pub fn jwt_claims(token: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
pub mod testing {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A unique temporary directory removed on drop.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(name: &str) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "aurora-control-test-{name}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }

        pub fn write(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn jwt(claims: &serde_json::Value) -> String {
        use base64::Engine;
        let encode = |value: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value);
        format!(
            "{}.{}.signature",
            encode(br#"{"alg":"none"}"#),
            encode(claims.to_string().as_bytes())
        )
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn tail_drops_the_partial_first_line() {
        let dir = TempDir::new("tail");
        let path = dir.write("log.jsonl", "first line\nsecond\nthird\n");
        assert_eq!(read_tail(&path, 13).unwrap(), "third\n");
        assert_eq!(
            read_tail(&path, 1000).unwrap(),
            "first line\nsecond\nthird\n"
        );
    }

    #[test]
    fn decodes_jwt_claims() {
        let token = jwt(&serde_json::json!({ "exp": 10, "email": "a@b.c" }));
        assert_eq!(jwt_claims(&token).unwrap()["exp"], 10);
        assert_eq!(jwt_claims("not-a-jwt"), None);
    }
}

#[cfg(test)]
mod limit_tests {
    use super::testing::TempDir;
    use super::*;

    #[test]
    fn i06_small_files_are_read_whole_and_large_ones_rejected_unread() {
        let dir = TempDir::new("limited");
        let small = dir.write("a.json", "{}");
        let large = dir.write("b.json", &"x".repeat(2048));
        assert_eq!(read_limited(&small, 16).as_deref(), Ok("{}"));
        assert_eq!(read_limited(&large, 1024), Err(ReadError::TooLarge));
        assert_eq!(
            read_limited(&dir.path().join("none.json"), 16),
            Err(ReadError::Missing)
        );
        let binary = dir.path().join("c.json");
        std::fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(read_limited(&binary, 16), Err(ReadError::Unreadable));
    }

    #[test]
    fn i06_the_budget_is_cumulative_across_files_bytes_and_records() {
        let dir = TempDir::new("budget");
        let first = dir.write("1.jsonl", &"line\n".repeat(100));
        let second = dir.write("2.jsonl", &"line\n".repeat(100));
        let mut budget = Budget::new(10, 2, 600, 3, Duration::from_secs(60));
        assert_eq!(
            read_tail_within(&first, 1000, &mut budget)
                .unwrap()
                .text
                .len(),
            500
        );
        // Only 100 bytes remain for the second file: its tail is truncated.
        let tail = read_tail_within(&second, 1000, &mut budget).unwrap();
        assert!(tail.cut, "the second file was cut by the budget");
        let tail = tail.text;
        assert!(tail.len() < 100, "{}", tail.len());
        assert!(budget.exhausted());
        assert!(read_tail_within(&first, 1000, &mut budget).is_err());

        let mut budget = Budget::new(10, 10, 10_000, 2, Duration::from_secs(60));
        assert!(budget.record() && budget.record());
        assert!(!budget.record());
        assert!(budget.exhausted());

        let mut budget = Budget::new(10, 10, 10_000, 10, Duration::ZERO);
        assert!(!budget.entry(), "the deadline also ends the scan");
    }
}
