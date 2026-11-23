mod demo;
mod model;

pub use demo::DemoAdapter;
pub use model::{Provider, Snapshot, Usage};

pub trait UsageAdapter: Send + Sync + 'static {
    fn snapshot(&self) -> Snapshot;
}

pub struct Service {
    adapter: Box<dyn UsageAdapter>,
}

impl Service {
    pub fn new(adapter: impl UsageAdapter) -> Self {
        Self {
            adapter: Box::new(adapter),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.adapter.snapshot()
    }
}

#[tauri::command]
pub fn get_usage(service: tauri::State<'_, Service>) -> Snapshot {
    service.snapshot()
}
