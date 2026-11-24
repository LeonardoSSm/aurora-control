//! Grok Build's own record of the account allowance.
//!
//! Grok Build (official xAI CLI, Apache-2.0) appends a structured line to
//! `$GROK_HOME/logs/unified.jsonl` every time it fetches the account's
//! credits: on start-up, after every prompt, on `/usage` and on its own poll
//! (`crates/codegen/xai-grok-shell/src/extensions/billing.rs`, event
//! `billing: fetched credits config`). The line's `ctx` is the same response
//! the CLI's `/usage` view draws: `{config, onDemandEnabled, subscriptionTier}`.
//!
//! This module only reads that file. It never writes, locks or truncates it,
//! never asks the CLI for anything, and never sends its content anywhere: the
//! adapter turns the newest record into normalized quotas.
//!
//! The file is shared by every Grok process (`O_APPEND` lines that may
//! interleave). Grok trims it in place to its newer half once it reaches
//! 5 MiB, and older builds replaced it by rename. So the reader:
//! - scans backwards from the end in fixed chunks, at most [`MAX_SCAN`] bytes,
//!   and stops at the first usable record;
//! - skips lines that are incomplete, malformed or longer than [`MAX_LINE`];
//! - keeps no offset between scans: a trimmed or replaced file is simply
//!   scanned again (the adapter caches by size and modification time).
//!
//! The billing line carries no account. The process that wrote it logged its
//! signed-in user earlier (`auth init user_info check`, `ctx.user_id`), so the
//! record's account is the one of the nearest earlier identity line of the
//! same `pid`. A record whose account cannot be found that way stays
//! unattributed, and the adapter does not show it.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;

pub const BILLING_EVENT: &str = "billing: fetched credits config";
pub const IDENTITY_EVENT: &str = "auth init user_info check";

/// Bytes read per step, from the end of the file.
const CHUNK: usize = 64 * 1024;
/// Grok trims the log once it reaches 5 MiB, so a scan of this size covers a
/// whole file. Larger files (an older build, or a foreign writer) are read
/// only this far from their end.
pub const MAX_SCAN: u64 = 6 * 1024 * 1024;
/// Longer lines are never parsed: a billing record is about 600 bytes.
pub const MAX_LINE: usize = 256 * 1024;
/// Wall-clock bound of one scan.
const MAX_TIME: Duration = Duration::from_millis(750);
/// A record dated this far in the future is not trusted (clock skew margin).
const FUTURE_MARGIN: TimeDelta = TimeDelta::minutes(5);

/// The newest billing record and the account of the process that wrote it.
#[derive(Clone, PartialEq)]
pub struct BillingRecord {
    pub observed_at: DateTime<Utc>,
    pub pid: u64,
    /// The record's `ctx`: `{config, onDemandEnabled, subscriptionTier}`.
    pub response: Value,
    /// The writer's signed-in user, in memory only. `None` when its identity
    /// line was not found within the scan.
    pub user_id: Option<String>,
}

impl std::fmt::Debug for BillingRecord {
    /// Never prints the user id.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BillingRecord")
            .field("observed_at", &self.observed_at)
            .field("pid", &self.pid)
            .field("attributed", &self.user_id.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Scan {
    /// No log file: Grok Build never ran with this home, or the file was just
    /// replaced.
    Missing,
    Unreadable,
    /// No usable billing record within the scanned part. `partial` when the
    /// scan stopped at a limit before the start of the file.
    Empty {
        partial: bool,
    },
    Found(BillingRecord),
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn moment(value: &Value) -> Option<DateTime<Utc>> {
    value
        .as_str()
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|time| time.with_timezone(&Utc))
}

/// A complete billing line, validated as far as this module can: its event,
/// a plausible time, a writer process and an object `config`.
fn billing(line: &[u8], now: DateTime<Utc>) -> Option<(u64, DateTime<Utc>, Value)> {
    if !contains(line, BILLING_EVENT.as_bytes()) {
        return None;
    }
    let mut value: Value = serde_json::from_slice(line).ok()?;
    if value["msg"].as_str() != Some(BILLING_EVENT) {
        return None;
    }
    let observed = moment(&value["ts"])?;
    if observed > now.checked_add_signed(FUTURE_MARGIN)? {
        return None;
    }
    let pid = value["pid"].as_u64()?;
    let context = value.get_mut("ctx")?.take();
    context.get("config").filter(|config| config.is_object())?;
    Some((pid, observed, context))
}

/// The signed-in user logged by process `pid`, from an identity line.
fn identity(line: &[u8], pid: u64) -> Option<String> {
    if !contains(line, IDENTITY_EVENT.as_bytes()) {
        return None;
    }
    let value: Value = serde_json::from_slice(line).ok()?;
    if value["msg"].as_str() != Some(IDENTITY_EVENT) || value["pid"].as_u64() != Some(pid) {
        return None;
    }
    value["ctx"]["user_id"]
        .as_str()
        .map(str::trim)
        .filter(|id| !id.is_empty() && id.len() <= 256)
        .map(str::to_owned)
}

/// What the backwards scan is looking for.
enum Wanted {
    Billing,
    Identity(BillingRecord),
}

/// Reads the newest billing record of the log at `path`.
pub fn latest(path: &Path, now: DateTime<Utc>) -> Scan {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Scan::Missing,
        Err(_) => return Scan::Unreadable,
    };
    match scan(&mut file, now) {
        Ok(scan) => scan,
        Err(_) => Scan::Unreadable,
    }
}

fn scan(file: &mut File, now: DateTime<Utc>) -> io::Result<Scan> {
    let length = file.metadata()?.len();
    let floor = length.saturating_sub(MAX_SCAN);
    let deadline = Instant::now() + MAX_TIME;
    let mut position = length;
    // The start of the line that continues past the chunk read last.
    let mut carry: Vec<u8> = Vec::new();
    // The line being assembled is longer than `MAX_LINE`: drop its pieces.
    let mut oversized = false;
    let mut wanted = Wanted::Billing;
    let mut chunk = vec![0u8; CHUNK];

    while position > floor {
        if Instant::now() >= deadline {
            break;
        }
        let step = (position - floor).min(CHUNK as u64) as usize;
        position -= step as u64;
        file.seek(SeekFrom::Start(position))?;
        file.read_exact(&mut chunk[..step])?;

        let mut buffer = Vec::with_capacity(step + carry.len());
        buffer.extend_from_slice(&chunk[..step]);
        buffer.extend_from_slice(&carry);
        carry.clear();

        let at_start = position == 0;
        let mut pieces: Vec<&[u8]> = buffer.split(|&byte| byte == b'\n').collect();
        // Before the first newline: the end of a line that starts further
        // back, unless this is the start of the file.
        let head = if at_start {
            None
        } else {
            Some(pieces.remove(0))
        };
        // The newest complete line of this buffer ends where the previous
        // carry ended; when that line was oversized, it is skipped once.
        let was_oversized = std::mem::take(&mut oversized);
        let mut skip_newest = was_oversized && !pieces.is_empty();
        for line in pieces.iter().rev() {
            if std::mem::take(&mut skip_newest) || line.len() > MAX_LINE {
                continue;
            }
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if let Wanted::Identity(record) = &mut wanted {
                if let Some(user) = identity(line, record.pid) {
                    record.user_id = Some(user);
                    break;
                }
            } else if let Some((pid, observed_at, response)) = billing(line, now) {
                wanted = Wanted::Identity(BillingRecord {
                    observed_at,
                    pid,
                    response,
                    user_id: None,
                });
            }
        }
        if let Wanted::Identity(record) = &wanted
            && record.user_id.is_some()
        {
            break;
        }
        if let Some(head) = head {
            // No newline in this chunk: still inside the oversized line.
            if (was_oversized && pieces.is_empty()) || head.len() > MAX_LINE {
                oversized = true;
            } else {
                carry.extend_from_slice(head);
            }
        }
    }

    Ok(match wanted {
        // Found, but the writer's identity line is gone (trimmed away).
        Wanted::Identity(record) => Scan::Found(record),
        Wanted::Billing => Scan::Empty {
            partial: position > 0,
        },
    })
}

/// Size and modification time: a scan is repeated only when they change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stamp {
    length: u64,
    modified: Option<std::time::SystemTime>,
}

pub fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(Stamp {
        length: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::local::testing::TempDir;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-24T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    /// The shape Grok Build 1.0.92 wrote on 2026-11-24, user id replaced.
    fn billing_line(ts: &str, pid: u64, percent: f64) -> String {
        format!(
            r#"{{"ts":"{ts}","src":"shell","pid":{pid},"ver":"1.0.92","lvl":"info","msg":"billing: fetched credits config","ctx":{{"config":{{"creditUsagePercent":{percent},"currentPeriod":{{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-11-20T03:30:32.116+00:00","end":"2026-11-27T03:30:32.116+00:00"}},"prepaidBalance":{{"val":0}},"isUnifiedBillingUser":true,"billingPeriodStart":"2026-11-20T03:30:32.116+00:00","billingPeriodEnd":"2026-11-27T03:30:32.116+00:00","historyLen":0}},"onDemandEnabled":null,"subscriptionTier":"SuperGrok Lite"}}}}"#
        )
    }

    fn identity_line(ts: &str, pid: u64, user: &str) -> String {
        format!(
            r#"{{"ts":"{ts}","src":"shell","pid":{pid},"ver":"1.0.92","lvl":"info","msg":"auth init user_info check","ctx":{{"user_id":"{user}","needs_user_info":false,"key_prefix":"sentinel-k","rt_prefix":"sentinel-r"}}}}"#
        )
    }

    fn noise(ts: &str, pid: u64) -> String {
        format!(
            r#"{{"ts":"{ts}","src":"shell","pid":{pid},"lvl":"info","msg":"startup timing","ctx":{{"name":"x","elapsed_ms":3}}}}"#
        )
    }

    fn write(dir: &TempDir, lines: &[String]) -> std::path::PathBuf {
        let mut text = lines.join("\n");
        text.push('\n');
        dir.write("unified.jsonl", &text)
    }

    fn found(scan: Scan) -> BillingRecord {
        match scan {
            Scan::Found(record) => record,
            other => panic!("expected a record, got {other:?}"),
        }
    }

    #[test]
    fn the_newest_record_is_attributed_to_the_identity_of_its_own_process() {
        let dir = TempDir::new("grok-log");
        let path = write(
            &dir,
            &[
                identity_line("2026-11-24T05:04:36Z", 5608, "user-a"),
                billing_line("2026-11-24T05:04:38Z", 5608, 71.0),
                identity_line("2026-11-24T11:37:00Z", 24352, "user-b"),
                noise("2026-11-24T11:37:01Z", 24352),
                billing_line("2026-11-24T11:37:08Z", 24352, 100.0),
                // A later process logged in but fetched nothing (for example
                // an ACP probe): it does not own the newest record.
                identity_line("2026-11-24T11:53:04Z", 9001, "user-c"),
                noise("2026-11-24T11:53:05Z", 9001),
            ],
        );
        let record = found(latest(&path, now()));
        assert_eq!(record.pid, 24352);
        assert_eq!(record.user_id.as_deref(), Some("user-b"));
        assert_eq!(record.response["config"]["creditUsagePercent"], 100.0);
        assert_eq!(record.response["subscriptionTier"], "SuperGrok Lite");
        assert_eq!(record.observed_at.to_rfc3339(), "2026-11-24T11:37:08+00:00");
        // Debug output never carries the account.
        assert!(!format!("{record:?}").contains("user-b"));
    }

    #[test]
    fn a_reused_pid_takes_the_nearest_earlier_identity() {
        let dir = TempDir::new("grok-log-pid");
        let path = write(
            &dir,
            &[
                identity_line("2026-11-24T01:00:00Z", 77, "old-account"),
                billing_line("2026-11-24T01:00:02Z", 77, 10.0),
                identity_line("2026-11-24T09:00:00Z", 77, "new-account"),
                billing_line("2026-11-24T09:00:02Z", 77, 20.0),
            ],
        );
        assert_eq!(
            found(latest(&path, now())).user_id.as_deref(),
            Some("new-account")
        );
    }

    #[test]
    fn partial_malformed_and_foreign_lines_are_skipped() {
        let dir = TempDir::new("grok-log-bad");
        let mut text = [
            identity_line("2026-11-24T05:00:00Z", 1, "user-a"),
            billing_line("2026-11-24T05:00:01Z", 1, 40.0),
            // Malformed JSON that mentions the event.
            r#"{"ts":"2026-11-24T05:00:02Z","pid":1,"msg":"billing: fetched credits config","ctx":{"#
                .into(),
            // The right event without a config object.
            r#"{"ts":"2026-11-24T05:00:03Z","pid":1,"msg":"billing: fetched credits config","ctx":{"config":null}}"#
                .into(),
            // Another message that merely quotes the event name.
            r#"{"ts":"2026-11-24T05:00:04Z","pid":1,"msg":"note","ctx":{"text":"billing: fetched credits config"}}"#
                .into(),
            // A record dated in the future.
            billing_line("2026-11-25T05:00:00Z", 1, 99.0),
        ]
        .join("\n");
        // A record still being written: no newline, cut mid-object.
        text.push('\n');
        let half = billing_line("2026-11-24T05:00:06Z", 1, 98.0);
        text.push_str(&half[..half.len() / 2]);
        let path = dir.write("unified.jsonl", &text);
        let record = found(latest(&path, now()));
        assert_eq!(record.response["config"]["creditUsagePercent"], 40.0);
        assert_eq!(record.user_id.as_deref(), Some("user-a"));
    }

    #[test]
    fn crlf_line_endings_are_accepted() {
        let dir = TempDir::new("grok-log-crlf");
        let text = format!(
            "{}\r\n{}\r\n",
            identity_line("2026-11-24T05:00:00Z", 3, "user-a"),
            billing_line("2026-11-24T05:00:01Z", 3, 55.0)
        );
        let path = dir.write("unified.jsonl", &text);
        assert_eq!(
            found(latest(&path, now())).response["config"]["creditUsagePercent"],
            55.0
        );
    }

    #[test]
    fn missing_empty_and_unattributed_logs() {
        let dir = TempDir::new("grok-log-none");
        assert_eq!(
            latest(&dir.path().join("unified.jsonl"), now()),
            Scan::Missing
        );
        let path = dir.write("unified.jsonl", "");
        assert_eq!(latest(&path, now()), Scan::Empty { partial: false });
        let path = write(&dir, &[noise("2026-11-24T05:00:00Z", 1)]);
        assert_eq!(latest(&path, now()), Scan::Empty { partial: false });
        // The writer's identity was trimmed away: found, but unattributed.
        let path = write(&dir, &[billing_line("2026-11-24T05:00:01Z", 4, 12.0)]);
        assert_eq!(found(latest(&path, now())).user_id, None);
        // An identity line of another process does not count.
        let path = write(
            &dir,
            &[
                identity_line("2026-11-24T05:00:00Z", 5, "user-x"),
                billing_line("2026-11-24T05:00:01Z", 4, 12.0),
            ],
        );
        assert_eq!(found(latest(&path, now())).user_id, None);
    }

    #[test]
    fn records_across_chunk_boundaries_and_oversized_lines() {
        let dir = TempDir::new("grok-log-chunks");
        let filler = "x".repeat(CHUNK - 100);
        let huge = format!(
            r#"{{"msg":"{BILLING_EVENT}","pad":"{}"}}"#,
            "y".repeat(MAX_LINE + 10)
        );
        let lines = [
            identity_line("2026-11-24T05:00:00Z", 8, "user-a"),
            noise("2026-11-24T05:00:00Z", 8).replace("x\"", &format!("{filler}\"")),
            billing_line("2026-11-24T05:00:01Z", 8, 33.0),
            noise("2026-11-24T05:00:02Z", 8).replace("x\"", &format!("{filler}\"")),
            // Longer than any record: skipped as a whole, even though it
            // spans several chunks and mentions the event.
            huge,
            noise("2026-11-24T05:00:03Z", 8),
        ];
        let path = write(&dir, &lines);
        let record = found(latest(&path, now()));
        assert_eq!(record.response["config"]["creditUsagePercent"], 33.0);
        assert_eq!(record.user_id.as_deref(), Some("user-a"));
    }

    #[test]
    fn a_large_log_is_read_only_up_to_the_scan_limit() {
        let dir = TempDir::new("grok-log-large");
        let mut text = String::new();
        text.push_str(&identity_line("2026-11-24T01:00:00Z", 9, "user-a"));
        text.push('\n');
        text.push_str(&billing_line("2026-11-24T01:00:01Z", 9, 61.0));
        text.push('\n');
        let line = noise("2026-11-24T02:00:00Z", 9);
        while (text.len() as u64) < MAX_SCAN + 512 * 1024 {
            text.push_str(&line);
            text.push('\n');
        }
        let path = dir.write("unified.jsonl", &text);
        // The only record lies beyond the limit: the scan says so instead of
        // reading the whole file.
        assert_eq!(latest(&path, now()), Scan::Empty { partial: true });
        // A record within the limit is found, and attributed when its
        // identity line is within the limit too.
        text.push_str(&identity_line("2026-11-24T03:00:00Z", 10, "user-b"));
        text.push('\n');
        text.push_str(&billing_line("2026-11-24T03:00:01Z", 10, 62.0));
        text.push('\n');
        let path = dir.write("unified.jsonl", &text);
        let record = found(latest(&path, now()));
        assert_eq!(record.user_id.as_deref(), Some("user-b"));
    }

    #[test]
    fn a_log_trimmed_or_replaced_between_scans_is_read_again() {
        let dir = TempDir::new("grok-log-trim");
        let path = write(
            &dir,
            &[
                identity_line("2026-11-24T05:00:00Z", 2, "user-a"),
                billing_line("2026-11-24T05:00:01Z", 2, 20.0),
                identity_line("2026-11-24T06:00:00Z", 3, "user-a"),
                billing_line("2026-11-24T06:00:01Z", 3, 30.0),
            ],
        );
        let before = stamp(&path).unwrap();
        assert_eq!(
            found(latest(&path, now())).response["config"]["creditUsagePercent"],
            30.0
        );
        // Grok keeps the newer half in place; a later record follows.
        let path = write(
            &dir,
            &[
                identity_line("2026-11-24T06:00:00Z", 3, "user-a"),
                billing_line("2026-11-24T07:00:01Z", 3, 31.0),
            ],
        );
        assert_ne!(stamp(&path), Some(before));
        assert_eq!(
            found(latest(&path, now())).response["config"]["creditUsagePercent"],
            31.0
        );
    }
}
