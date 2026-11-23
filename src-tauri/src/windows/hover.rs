//! Hover state shared by the rail and the summary.
//!
//! - Opening waits for an intent delay, so sweeping the cursor along the
//!   screen edge does not open indicators by accident. Measured on Windows 11:
//!   a 52 px indicator is crossed in about 100 ms at 500 px/s.
//! - Switching to the other indicator also waits for intent; the current panel
//!   stays open meanwhile, so nothing flickers.
//! - Closing is decided from the real cursor position: the panel stays open
//!   while the cursor is on the expanded circle, the summary, or the gap
//!   between them, and closes after a grace period outside that zone. This
//!   also recovers from pointer-leave events the WebView never delivers.
//! - The summary handshake (`ready`, revisions) keeps a late render
//!   confirmation from reopening a selection that was already invalidated.

use std::time::{Duration, Instant};

use super::Provider;

pub const OPEN_DELAY: Duration = Duration::from_millis(150);
pub const SWITCH_DELAY: Duration = Duration::from_millis(120);
pub const CLOSE_GRACE: Duration = Duration::from_millis(240);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Selection {
    pub provider: Provider,
    pub revision: u32,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum Action {
    Nothing,
    /// Re-check after the delay with this epoch.
    OpenAfter(u32, Duration),
}

#[derive(Default)]
pub(super) struct Hover {
    pointer: Option<Provider>,
    active: Option<Provider>,
    epoch: u32,
    outside_since: Option<Instant>,
    ready: bool,
    revision: u32,
    selected: Option<Selection>,
}

impl Hover {
    pub fn active(&self) -> Option<Provider> {
        self.active
    }

    /// The indicator under the pointer, as reported by the rail.
    pub fn pointer(&mut self, provider: Option<Provider>) -> Action {
        self.pointer = provider;
        self.epoch = self.epoch.wrapping_add(1);
        match (provider, self.active) {
            (Some(_), None) => Action::OpenAfter(self.epoch, OPEN_DELAY),
            (Some(provider), Some(active)) if provider != active => {
                Action::OpenAfter(self.epoch, SWITCH_DELAY)
            }
            _ => Action::Nothing,
        }
    }

    /// Fires when the intent delay elapses: the pointer stayed on an indicator
    /// that is not the active one.
    pub fn intent_elapsed(&self, epoch: u32) -> Option<Provider> {
        if epoch != self.epoch {
            return None;
        }
        self.pointer
            .filter(|provider| Some(*provider) != self.active)
    }

    /// Marks `provider` as active and returns the summary selection to publish.
    pub fn activate(&mut self, provider: Option<Provider>) -> Option<Selection> {
        self.active = provider;
        self.outside_since = None;
        if provider.is_none() {
            self.pointer = None;
        }
        self.select(provider)
    }

    /// Called periodically while active. Returns true when the panel must close.
    pub fn tick(&mut self, inside_zone: bool, now: Instant) -> bool {
        if self.active.is_none() || inside_zone {
            self.outside_since = None;
            return false;
        }
        let since = *self.outside_since.get_or_insert(now);
        now.duration_since(since) >= CLOSE_GRACE
    }

    fn select(&mut self, provider: Option<Provider>) -> Option<Selection> {
        self.revision = self.revision.wrapping_add(1);
        self.selected = provider.map(|provider| Selection {
            provider,
            revision: self.revision,
        });
        self.selected.filter(|_| self.ready)
    }

    /// The summary registered its listener.
    pub fn ready(&mut self) -> Option<Selection> {
        self.ready = true;
        self.selected
    }

    /// The current selection, for republishing fresh data without a new revision.
    pub fn current(&self) -> Option<Selection> {
        self.selected.filter(|_| self.ready)
    }

    pub fn confirm(&self, revision: u32) -> Option<Provider> {
        self.selected
            .filter(|selection| self.ready && selection.revision == revision)
            .map(|selection| selection.provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_waits_for_intent_and_ignores_a_passing_cursor() {
        let mut hover = Hover::default();
        let Action::OpenAfter(first, OPEN_DELAY) = hover.pointer(Some(Provider::Codex)) else {
            panic!("expected an intent delay");
        };
        // The cursor leaves, or sweeps on to the next indicator, before the delay.
        assert_eq!(hover.pointer(None), Action::Nothing);
        assert_eq!(hover.intent_elapsed(first), None);
        let Action::OpenAfter(passing, _) = hover.pointer(Some(Provider::Codex)) else {
            panic!("expected an intent delay");
        };
        let Action::OpenAfter(second, _) = hover.pointer(Some(Provider::ClaudeCode)) else {
            panic!("expected an intent delay");
        };
        assert_eq!(hover.intent_elapsed(passing), None);
        assert_eq!(hover.intent_elapsed(second), Some(Provider::ClaudeCode));
    }

    #[test]
    fn switching_between_indicators_also_waits_for_intent() {
        let mut hover = Hover::default();
        hover.activate(Some(Provider::Codex));
        assert_eq!(hover.pointer(Some(Provider::Codex)), Action::Nothing);
        assert_eq!(hover.pointer(None), Action::Nothing);
        let Action::OpenAfter(epoch, SWITCH_DELAY) = hover.pointer(Some(Provider::ClaudeCode))
        else {
            panic!("expected a switch delay");
        };
        assert_eq!(
            hover.active(),
            Some(Provider::Codex),
            "no flicker meanwhile"
        );
        assert_eq!(hover.intent_elapsed(epoch), Some(Provider::ClaudeCode));
        hover.activate(Some(Provider::ClaudeCode));
        assert_eq!(hover.intent_elapsed(epoch), None, "already active");
    }

    #[test]
    fn closes_only_after_the_grace_period_outside_the_zone() {
        let mut hover = Hover::default();
        let start = Instant::now();
        assert!(!hover.tick(false, start), "nothing active");
        hover.activate(Some(Provider::Codex));
        assert!(!hover.tick(false, start));
        // Crossing the gap to the summary resets the timer.
        assert!(!hover.tick(true, start + Duration::from_millis(200)));
        assert!(!hover.tick(false, start + Duration::from_millis(300)));
        assert!(!hover.tick(false, start + Duration::from_millis(500)));
        assert!(hover.tick(false, start + Duration::from_millis(540)));
    }

    #[test]
    fn waits_for_the_summary_listener_and_preserves_the_latest_selection() {
        let mut hover = Hover::default();
        assert_eq!(hover.activate(Some(Provider::Codex)), None);
        assert_eq!(hover.activate(Some(Provider::ClaudeCode)), None);
        let selection = hover.ready().unwrap();
        assert_eq!(selection.provider, Provider::ClaudeCode);
        assert_eq!(
            hover.confirm(selection.revision),
            Some(Provider::ClaudeCode)
        );
        assert_eq!(hover.current(), Some(selection));
    }

    #[test]
    fn a_new_selection_or_closing_invalidates_old_render_confirmations() {
        let mut hover = Hover::default();
        assert_eq!(hover.ready(), None);
        let first = hover.activate(Some(Provider::Codex)).unwrap();
        let second = hover.activate(Some(Provider::ClaudeCode)).unwrap();
        assert_eq!(hover.confirm(first.revision), None);
        hover.activate(None);
        assert_eq!(hover.confirm(second.revision), None);
        assert_eq!(hover.current(), None);
        assert_eq!(hover.active(), None);
    }
}
