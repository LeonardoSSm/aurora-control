//! Read-only access to files written by the providers' own clients.
//! Files are opened with the default Windows share mode (read, write and
//! delete), so the providers can keep writing and rotating them.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

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

/// Reads at most `max` bytes from the end of a file. When the read starts in
/// the middle of the file, the first partial line is dropped.
pub fn read_tail(path: &Path, max: u64) -> io::Result<String> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(max);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((length - start) as usize);
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
