use super::UsageAdapter;
use super::model::{ConsumptionWindow, Provider, Reading, Snapshot, Source, Usage};

pub struct DemoAdapter;

impl UsageAdapter for DemoAdapter {
    fn snapshot(&self) -> Snapshot {
        [
            (Provider::Codex, 64, "2026-11-23T20:00:00Z"),
            (Provider::ClaudeCode, 37, "2026-11-23T21:00:00Z"),
        ]
        .into_iter()
        .map(|(provider, percent_used, resets_at)| Usage {
            provider,
            account: Some("Conta de exemplo".into()),
            source: Source::Demo,
            reading: Reading::Available { percent_used },
            window: Some(ConsumptionWindow {
                duration_minutes: 300,
                resets_at: Some(resets_at.into()),
            }),
            updated_at: Some("2026-11-23T17:00:00Z".into()),
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn demo_serializes_the_wire_contract() {
        let snapshot = serde_json::to_value(DemoAdapter.snapshot()).unwrap();
        assert_eq!(
            snapshot[0],
            json!({
                "provider": "codex",
                "account": "Conta de exemplo",
                "source": "demo",
                "status": "available",
                "percentUsed": 64,
                "window": { "durationMinutes": 300, "resetsAt": "2026-11-23T20:00:00Z" },
                "updatedAt": "2026-11-23T17:00:00Z"
            })
        );
        assert_eq!(snapshot[1]["provider"], "claudeCode");
        assert_eq!(snapshot[1]["source"], "demo");
        assert_eq!(snapshot[1]["percentUsed"], 37);
    }
}
