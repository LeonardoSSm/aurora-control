//! Usage service: one background worker per provider adapter. Workers refresh
//! on their own schedule; the IPC only reads the cached state, so hover and
//! animations never wait for the network.
//!
//! Providers are isolated from each other: each one has its own adapter,
//! tracker (cache, retry state and identity), refresh policy and worker
//! thread. A failure, a panic or a long `Retry-After` of one provider never
//! delays or alters another.

mod adapter;
pub mod catalog;
mod claude;
mod codex;
mod demo;
mod gemini;
mod grok;
mod http;
mod local;
mod model;
mod scheduler;
mod session;
mod time;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub use adapter::UsageAdapter;
#[cfg(test)]
pub use local::testing;
pub use local::{ReadError, read_limited};
#[cfg(test)]
pub use model::{ConsumptionWindow, Quota};
pub use model::{
    Diagnostics, Issue, Provider, QuotaKind, Reading, Snapshot, Source, Usage, UsageUpdate,
};
pub use session::Fingerprint;

use crate::sync::{lock, wait_timeout_while};
use adapter::{FetchError, Outcome};
use scheduler::{Now, Policy, Tracker};

type Notify = Box<dyn Fn() + Send + Sync>;

/// Longest sleep of a worker: an upper bound for clock-change recovery.
const MAX_SLEEP: Duration = Duration::from_secs(300);
const MIN_SLEEP: Duration = Duration::from_millis(250);

struct Slot {
    adapter: Box<dyn UsageAdapter>,
    tracker: Mutex<Tracker>,
    manual: Mutex<bool>,
    /// Hidden providers are neither queried nor read from disk.
    enabled: AtomicBool,
    wake: Condvar,
}

/// What the alert engine sees of one provider: its published state, whether
/// it is enabled, and the process-local identity of its verified account.
/// Never serialized.
#[derive(Clone, Debug)]
pub struct Observation {
    pub enabled: bool,
    pub identity: Option<Fingerprint>,
    pub usage: Usage,
}

pub struct Service {
    slots: Vec<Arc<Slot>>,
    /// Publication counter. Snapshots are taken while it is held, so a higher
    /// sequence always carries newer data (audit I03).
    sequence: Mutex<u64>,
}

impl Service {
    pub fn new(adapters: Vec<Box<dyn UsageAdapter>>) -> Self {
        Self::build(adapters, Policy::for_provider)
    }

    #[cfg(test)]
    fn with_policy(adapters: Vec<Box<dyn UsageAdapter>>, policy: Policy) -> Self {
        Self::build(adapters, |_| policy)
    }

    fn build(adapters: Vec<Box<dyn UsageAdapter>>, policy: impl Fn(Provider) -> Policy) -> Self {
        let now = Now::current();
        Self {
            slots: adapters
                .into_iter()
                .map(|adapter| {
                    let provider = adapter.provider();
                    Arc::new(Slot {
                        tracker: Mutex::new(Tracker::new(
                            provider,
                            adapter.source(),
                            policy(provider),
                            now,
                        )),
                        adapter,
                        manual: Mutex::new(false),
                        // Until the preferences say otherwise, only the
                        // providers enabled by default may read anything.
                        enabled: AtomicBool::new(catalog::entry(provider).default_enabled),
                        wake: Condvar::new(),
                    })
                })
                .collect(),
            sequence: Mutex::new(0),
        }
    }

    /// Live adapters, or the fixture when `AURORA_DEMO` is set.
    pub fn from_environment() -> Self {
        let demo = std::env::var("AURORA_DEMO").ok();
        match demo::DemoAdapter::from_env(demo.as_deref()) {
            Some(adapters) => Self::new(
                adapters
                    .into_iter()
                    .map(|adapter| Box::new(adapter) as Box<dyn UsageAdapter>)
                    .collect(),
            ),
            None => Self::new(vec![
                Box::new(codex::Codex::detect()),
                Box::new(claude::ClaudeCode::detect()),
                Box::new(gemini::Gemini::detect()),
                Box::new(grok::Grok::detect()),
            ]),
        }
    }

    pub fn is_demo(&self) -> bool {
        self.slots
            .iter()
            .any(|slot| slot.adapter.source() == model::Source::Demo)
    }

    pub fn snapshot(&self) -> Snapshot {
        let now = Now::current();
        self.slots
            .iter()
            .map(|slot| lock(&slot.tracker).usage(now))
            .collect()
    }

    /// The current snapshot with the next publication number.
    pub fn update(&self) -> UsageUpdate {
        let mut sequence = lock(&self.sequence);
        *sequence = sequence.saturating_add(1);
        UsageUpdate {
            sequence: *sequence,
            snapshot: self.snapshot(),
        }
    }

    /// Starts one worker per provider. `notify` runs after every state change.
    pub fn start(&self, notify: Notify) {
        let notify: Arc<Notify> = Arc::new(notify);
        for slot in &self.slots {
            let slot = Arc::clone(slot);
            let notify = Arc::clone(&notify);
            std::thread::Builder::new()
                .name(format!("usage-{:?}", slot.adapter.provider()))
                .spawn(move || worker(&slot, notify.as_ref()))
                .expect("failed to start a usage worker");
        }
    }

    /// Requests an immediate refresh. The recovery rules still apply.
    pub fn refresh(&self, provider: Option<Provider>) {
        for slot in &self.slots {
            if provider.is_none_or(|provider| provider == slot.adapter.provider()) {
                *lock(&slot.manual) = true;
                slot.wake.notify_one();
            }
        }
    }

    /// Shows only `visible` providers. Hidden ones stop querying and reading
    /// local files; shown again, they refresh at once.
    pub fn set_visible(&self, visible: &[Provider]) {
        for slot in &self.slots {
            let show = visible.contains(&slot.adapter.provider());
            let was = slot.enabled.swap(show, Ordering::SeqCst);
            if show && !was {
                *lock(&slot.manual) = true;
                slot.wake.notify_one();
            }
        }
    }

    /// Chooses the ring's metric of each provider (`None`: automatic).
    /// Presentation only: nothing is queried again.
    pub fn select_metrics(&self, metrics: &[(Provider, Option<String>)]) {
        for slot in &self.slots {
            let provider = slot.adapter.provider();
            if let Some((_, metric)) = metrics.iter().find(|(id, _)| *id == provider) {
                lock(&slot.tracker).select_metric(metric.clone());
            }
        }
    }

    /// The state of every provider, for the alert engine. Built from the same
    /// trackers as the published snapshot: no provider is queried for it.
    pub fn observations(&self) -> Vec<Observation> {
        let now = Now::current();
        self.slots
            .iter()
            .map(|slot| {
                let tracker = lock(&slot.tracker);
                Observation {
                    enabled: slot.enabled.load(Ordering::SeqCst),
                    identity: tracker.identity(),
                    // Independent of the ring metric (a presentation choice).
                    usage: tracker.automatic_usage(now),
                }
            })
            .collect()
    }

    /// The sign-in diagnosis of `provider`, with its latest issue.
    pub fn diagnostics(&self, provider: Provider) -> Option<Diagnostics> {
        let slot = self
            .slots
            .iter()
            .find(|slot| slot.adapter.provider() == provider)
            // A hidden provider has no files read, not even for the diagnosis.
            .filter(|slot| slot.enabled.load(Ordering::SeqCst))?;
        let now = Now::current();
        let rows = guarded(|| slot.adapter.diagnostics(now.utc)).flatten()?;
        let usage = lock(&slot.tracker).usage(now);
        Some(Diagnostics {
            provider,
            rows,
            issue: usage.issue,
            checked_at: usage.checked_at,
        })
    }
}

/// Runs `job`, turning a panic into `None`. Nothing inside a job may unwind
/// past the worker loop: a panic there would end the worker for good.
fn guarded<T>(job: impl FnOnce() -> T) -> Option<T> {
    catch_unwind(AssertUnwindSafe(job)).ok()
}

/// How long a worker sleeps before looking again. A hidden provider never
/// starts an attempt, so its next wake-up stays overdue: it sleeps the
/// longest instead of spinning at the shortest; `set_visible` wakes it.
fn sleep_for(enabled: bool, next: std::time::Instant, now: std::time::Instant) -> Duration {
    if enabled {
        next.saturating_duration_since(now)
            .clamp(MIN_SLEEP, MAX_SLEEP)
    } else {
        MAX_SLEEP
    }
}

fn worker(slot: &Slot, notify: &Notify) {
    // The last state announced, to publish transitions that happen without a
    // network result (renewals, ageing, a logout seen by the probe).
    let mut announced: Option<Usage> = None;
    loop {
        let manual = {
            let wait = {
                let now = Now::current();
                let next = lock(&slot.tracker).next_wake(now);
                sleep_for(slot.enabled.load(Ordering::SeqCst), next, now.instant)
            };
            let manual = lock(&slot.manual);
            let mut manual = wait_timeout_while(&slot.wake, manual, wait, |manual| !*manual);
            std::mem::take(&mut *manual)
        };

        let now = Now::current();
        if !slot.enabled.load(Ordering::SeqCst) {
            continue;
        }
        // Local files only: a logout, a switch or a renewed credential is
        // noticed before the gates are evaluated.
        let probe = guarded(|| slot.adapter.session(now.utc)).flatten();
        let current = {
            let mut tracker = lock(&slot.tracker);
            if let Some(session) = probe {
                tracker.probe(session, now);
            }
            if tracker.should_fetch(now, manual) {
                tracker.begin(now);
                None
            } else {
                Some(tracker.usage(now))
            }
        };
        if let Some(current) = current {
            if announced.as_ref() != Some(&current) {
                announced = Some(current);
                guarded(notify);
            }
            continue;
        }

        guarded(notify);
        // A panicking adapter (for example on a response nobody anticipated)
        // ends this attempt as malformed. The worker keeps running.
        let outcome = guarded(|| slot.adapter.fetch(now.utc)).unwrap_or_else(|| {
            eprintln!(
                "Aurora Control: the {:?} adapter panicked",
                slot.adapter.provider()
            );
            Outcome::failed(FetchError::Malformed("adapter panicked".into()))
        });
        {
            let mut tracker = lock(&slot.tracker);
            let now = Now::current();
            if guarded(|| tracker.finish(outcome, now)).is_none() {
                tracker.abort(now);
            }
            announced = Some(tracker.usage(now));
        }
        guarded(notify);
    }
}

#[tauri::command]
pub fn get_usage(service: tauri::State<'_, Service>) -> UsageUpdate {
    service.update()
}

#[tauri::command]
pub fn refresh_usage(service: tauri::State<'_, Service>, provider: Option<Provider>) {
    service.refresh(provider);
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::mpsc;

    #[test]
    fn demo_serializes_the_wire_contract() {
        let service = Service::new(
            demo::DemoAdapter::from_env(Some("64,37"))
                .unwrap()
                .into_iter()
                .map(|adapter| Box::new(adapter) as Box<dyn UsageAdapter>)
                .collect(),
        );
        let (sender, receiver) = mpsc::channel();
        let sender = Mutex::new(sender);
        service.start(Box::new(move || {
            let _ = sender.lock().unwrap().send(());
        }));
        // Two notifications per provider: start and finish of the first fetch.
        for _ in 0..4 {
            receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        }

        let snapshot = serde_json::to_value(service.snapshot()).unwrap();
        let codex = &snapshot[0];
        assert_eq!(codex["provider"], "codex");
        assert_eq!(codex["source"], "demo");
        assert_eq!(codex["origin"], "fixture");
        assert_eq!(codex["status"], "available");
        assert_eq!(codex["percentUsed"], 64);
        assert_eq!(codex["account"], "Conta de exemplo");
        assert_eq!(codex["window"]["id"], "codex-primary");
        assert_eq!(codex["window"]["durationMinutes"], 300);
        assert_eq!(codex["quotas"][0]["kind"], "window");
        assert_eq!(codex["quotas"][0]["renewed"], false);
        assert_eq!(codex["window"]["renewed"], false);
        assert!(
            codex["quotas"][0].get("eligible").is_none(),
            "internal flag stays in Rust"
        );
        assert_eq!(codex["issue"], json!(null));
        assert_eq!(codex["refreshing"], false);
        assert!(codex["updatedAt"].as_str().unwrap().ends_with('Z'));

        let claude = &snapshot[1];
        assert_eq!(claude["provider"], "claudeCode");
        assert_eq!(claude["percentUsed"], 37);
        assert_eq!(
            claude["activity"],
            json!({ "windowMinutes": 300, "tokens": 184250, "sessions": 2, "partial": false })
        );
    }

    /// Panics on the first attempt with an out-of-range value, then returns
    /// the demo reading.
    struct Flaky {
        calls: Mutex<u32>,
        inner: demo::DemoAdapter,
    }

    impl UsageAdapter for Flaky {
        fn provider(&self) -> Provider {
            Provider::Codex
        }

        fn fetch(&self, now: chrono::DateTime<chrono::Utc>) -> Outcome {
            let call = {
                let mut calls = lock(&self.calls);
                *calls += 1;
                *calls
            };
            if call == 1 {
                // The baseline panicked here: TimeDelta::seconds out of range.
                let _ = chrono::TimeDelta::seconds(i64::MAX);
            }
            self.inner.fetch(now)
        }
    }

    #[test]
    fn b02_a_panicking_attempt_neither_kills_the_worker_nor_poisons_the_cache() {
        let service = Service::with_policy(
            vec![Box::new(Flaky {
                calls: Mutex::new(0),
                inner: demo::DemoAdapter::new(Provider::Codex, 42),
            })],
            Policy {
                manual_gap: Duration::ZERO,
                backoff_min: Duration::ZERO,
                ..Policy::default()
            },
        );
        let (sender, receiver) = mpsc::channel();
        let sender = Mutex::new(sender);
        service.start(Box::new(move || {
            let _ = lock(&sender).send(());
        }));
        // Start and end of the aborted attempt.
        for _ in 0..2 {
            receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        let usage = &service.snapshot()[0];
        assert!(!usage.refreshing, "refreshing must end");
        assert_eq!(usage.issue, Some(model::Issue::Malformed));
        assert!(usage.checked_at.is_some());

        // The same worker serves the next request.
        service.refresh(None);
        for _ in 0..2 {
            receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        let usage = &service.snapshot()[0];
        assert_eq!(usage.issue, None);
        assert_eq!(
            usage.reading,
            model::Reading::Available { percent_used: 42 }
        );
    }

    #[test]
    fn i03_publication_sequences_strictly_increase_across_threads() {
        let service = Arc::new(Service::new(
            demo::DemoAdapter::from_env(Some("10,20"))
                .unwrap()
                .into_iter()
                .map(|adapter| Box::new(adapter) as Box<dyn UsageAdapter>)
                .collect(),
        ));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let service = Arc::clone(&service);
                std::thread::spawn(move || {
                    (0..50)
                        .map(|_| service.update().sequence)
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut all: Vec<u64> = threads
            .into_iter()
            .flat_map(|thread| thread.join().unwrap())
            .collect();
        let count = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), count, "no sequence is published twice");
        assert_eq!(*lock(&service.sequence), count as u64);
    }
    /// Counts diagnosis reads, which touch the provider's local files.
    struct Diagnosed {
        reads: Arc<Mutex<u32>>,
    }

    impl UsageAdapter for Diagnosed {
        fn provider(&self) -> Provider {
            Provider::ClaudeCode
        }
        fn fetch(&self, _now: chrono::DateTime<chrono::Utc>) -> Outcome {
            Outcome::failed(FetchError::AuthMissing)
        }
        fn diagnostics(
            &self,
            _now: chrono::DateTime<chrono::Utc>,
        ) -> Option<Vec<model::DiagnosticRow>> {
            *lock(&self.reads) += 1;
            Some(vec![])
        }
    }

    #[test]
    fn a_hidden_provider_is_not_diagnosed() {
        let reads = Arc::new(Mutex::new(0));
        let service = Service::new(vec![Box::new(Diagnosed {
            reads: Arc::clone(&reads),
        })]);
        assert!(service.diagnostics(Provider::ClaudeCode).is_some());
        assert_eq!(*lock(&reads), 1);
        service.set_visible(&[Provider::Codex]);
        assert!(service.diagnostics(Provider::ClaudeCode).is_none());
        assert_eq!(*lock(&reads), 1, "no file read while hidden");
        service.set_visible(&[Provider::ClaudeCode]);
        assert!(service.diagnostics(Provider::ClaudeCode).is_some());
    }

    /// Counts every local probe and fetch, which read credentials, transcripts
    /// or the network.
    struct Counted {
        probes: Arc<Mutex<u32>>,
        fetches: Arc<Mutex<u32>>,
        inner: demo::DemoAdapter,
    }

    impl UsageAdapter for Counted {
        fn provider(&self) -> Provider {
            Provider::ClaudeCode
        }
        fn fetch(&self, now: chrono::DateTime<chrono::Utc>) -> Outcome {
            *lock(&self.fetches) += 1;
            self.inner.fetch(now)
        }
        fn session(&self, _now: chrono::DateTime<chrono::Utc>) -> Option<session::Session> {
            *lock(&self.probes) += 1;
            None
        }
    }

    #[test]
    fn a_hidden_worker_reads_nothing_until_shown_again() {
        let (probes, fetches) = (Arc::new(Mutex::new(0)), Arc::new(Mutex::new(0)));
        let service = Service::with_policy(
            vec![Box::new(Counted {
                probes: Arc::clone(&probes),
                fetches: Arc::clone(&fetches),
                inner: demo::DemoAdapter::new(Provider::ClaudeCode, 30),
            })],
            Policy {
                manual_gap: Duration::ZERO,
                backoff_min: Duration::ZERO,
                ..Policy::default()
            },
        );
        // Hidden before the worker starts, as `windows::setup` does.
        service.set_visible(&[Provider::Codex]);
        let (sender, receiver) = mpsc::channel();
        let sender = Mutex::new(sender);
        service.start(Box::new(move || {
            let _ = lock(&sender).send(());
        }));
        // Manual refreshes while hidden wake the worker but read nothing.
        for _ in 0..3 {
            service.refresh(None);
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            receiver.try_recv().is_err(),
            "nothing published while hidden"
        );
        assert_eq!((*lock(&probes), *lock(&fetches)), (0, 0));

        // Shown again: it probes and refreshes at once.
        service.set_visible(&[Provider::ClaudeCode]);
        for _ in 0..2 {
            receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        assert!(*lock(&probes) >= 1);
        assert_eq!(*lock(&fetches), 1);
        assert_eq!(
            service.snapshot()[0].reading,
            model::Reading::Available { percent_used: 30 }
        );
    }

    #[test]
    fn a_hidden_worker_with_an_overdue_refresh_sleeps_instead_of_spinning() {
        let now = std::time::Instant::now();
        let overdue = now.checked_sub(Duration::from_secs(60)).unwrap_or(now);
        // Before: an overdue wake-up kept a hidden worker at MIN_SLEEP.
        assert_eq!(sleep_for(true, overdue, now), MIN_SLEEP);
        assert_eq!(sleep_for(false, overdue, now), MAX_SLEEP);
        assert_eq!(
            sleep_for(true, now + Duration::from_secs(30), now),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn issues_serialize_with_a_code_tag() {
        let issue = model::Issue::RateLimited {
            retry_at: Some("2026-11-23T18:10:00Z".into()),
        };
        assert_eq!(
            serde_json::to_value(issue).unwrap(),
            json!({ "code": "rateLimited", "retryAt": "2026-11-23T18:10:00Z" })
        );
        assert_eq!(
            serde_json::to_value(model::Issue::Http {
                status: 503,
                retry_at: None
            })
            .unwrap(),
            json!({ "code": "http", "status": 503, "retryAt": null })
        );
    }
}
