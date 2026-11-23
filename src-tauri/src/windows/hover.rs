use super::Provider;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Selection {
    pub provider: Provider,
    pub revision: u32,
}

#[derive(Default)]
pub(super) struct Hover {
    ready: bool,
    revision: u32,
    selected: Option<Selection>,
}

impl Hover {
    pub fn select(&mut self, provider: Option<Provider>) -> Option<Selection> {
        self.revision = self.revision.wrapping_add(1);
        self.selected = provider.map(|provider| Selection {
            provider,
            revision: self.revision,
        });
        self.selected.filter(|_| self.ready)
    }

    pub fn ready(&mut self) -> Option<Selection> {
        self.ready = true;
        self.selected
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
    fn waits_for_the_summary_listener_and_preserves_the_latest_hover() {
        let mut hover = Hover::default();
        assert_eq!(hover.select(Some(Provider::Codex)), None);
        assert_eq!(hover.select(Some(Provider::ClaudeCode)), None);
        let selection = hover.ready().unwrap();
        assert_eq!(selection.provider, Provider::ClaudeCode);
        assert_eq!(
            hover.confirm(selection.revision),
            Some(Provider::ClaudeCode)
        );
    }

    #[test]
    fn entering_another_indicator_or_leaving_invalidates_old_render_confirmations() {
        let mut hover = Hover::default();
        assert_eq!(hover.ready(), None);
        let first = hover.select(Some(Provider::Codex)).unwrap();
        let second = hover.select(Some(Provider::ClaudeCode)).unwrap();
        assert_eq!(hover.confirm(first.revision), None);
        hover.select(None);
        assert_eq!(hover.confirm(second.revision), None);
    }
}
