//! Usage service: one background worker per provider adapter. Workers refresh
//! on their own schedule; the IPC only reads the cached state, so hover and
//! animations never wait for the network.

mod adapter;
mod claude;
mod codex;
mod demo;
mod http;
mod local;
mod model;
mod scheduler;

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use chrono::Utc;

pub use adapter::UsageAdapter;
pub use model::{Provider, Snapshot, Usage};

use scheduler::{Policy, Tracker};

type Notify = Box<dyn Fn() + Send + Sync>;

struct Slot {
    adapter: Box<dyn UsageAdapter>,
    tracker: Mutex<Tracker>,
    manual: Mutex<bool>,
    wake: Condvar,
}

pub struct Service {
    slots: Vec<Arc<Slot>>,
}

impl Service {
    pub fn new(adapters: Vec<Box<dyn UsageAdapter>>) -> Self {
        let now = Utc::now();
        let policy = Policy::default();
        Self {
            slots: adapters
                .into_iter()
                .map(|adapter| {
                    Arc::new(Slot {
                        tracker: Mutex::new(Tracker::new(
                            adapter.provider(),
                            adapter.source(),
                            policy,
                            now,
                        )),
                        adapter,
                        manual: Mutex::new(false),
                        wake: Condvar::new(),
                    })
                })
                .collect(),
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
            ]),
        }
    }

    pub fn is_demo(&self) -> bool {
        self.slots
            .iter()
            .any(|slot| slot.adapter.source() == model::Source::Demo)
    }

    pub fn snapshot(&self) -> Snapshot {
        let now = Utc::now();
        self.slots
            .iter()
            .map(|slot| slot.tracker.lock().unwrap().usage(now))
            .collect()
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

    /// Requests an immediate refresh. Debounce and `Retry-After` still apply.
    pub fn refresh(&self, provider: Option<Provider>) {
        for slot in &self.slots {
            if provider.is_none_or(|provider| provider == slot.adapter.provider()) {
                *slot.manual.lock().unwrap() = true;
                slot.wake.notify_one();
            }
        }
    }
}

fn worker(slot: &Slot, notify: &Notify) {
    loop {
        let manual = {
            let wait = {
                let tracker = slot.tracker.lock().unwrap();
                (tracker.next_due() - Utc::now())
                    .to_std()
                    .unwrap_or(Duration::ZERO)
                    .clamp(Duration::from_millis(250), Duration::from_secs(300))
            };
            let manual = slot.manual.lock().unwrap();
            let (mut manual, _) = slot
                .wake
                .wait_timeout_while(manual, wait, |manual| !*manual)
                .unwrap();
            std::mem::take(&mut *manual)
        };

        let now = Utc::now();
        {
            let mut tracker = slot.tracker.lock().unwrap();
            if !tracker.should_fetch(now, manual) {
                continue;
            }
            tracker.begin(now);
        }
        notify();
        let outcome = slot.adapter.fetch(Utc::now());
        slot.tracker.lock().unwrap().finish(outcome, Utc::now());
        notify();
    }
}

#[tauri::command]
pub fn get_usage(service: tauri::State<'_, Service>) -> Snapshot {
    service.snapshot()
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
        assert_eq!(codex["issue"], json!(null));
        assert_eq!(codex["refreshing"], false);
        assert!(codex["updatedAt"].as_str().unwrap().ends_with('Z'));

        let claude = &snapshot[1];
        assert_eq!(claude["provider"], "claudeCode");
        assert_eq!(claude["percentUsed"], 37);
        assert_eq!(
            claude["activity"],
            json!({ "windowMinutes": 300, "tokens": 184250, "sessions": 2 })
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
            serde_json::to_value(model::Issue::Http { status: 503 }).unwrap(),
            json!({ "code": "http", "status": 503 })
        );
    }
}
