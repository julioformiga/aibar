use crate::config::cooldown;
use crate::model::{Provider, SourceState};
use crate::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

pub enum Action {
    Quit,
}

pub enum RefreshResult {
    Triggered,
    #[allow(dead_code)]
    CooldownActive {
        secs_remaining: u32,
    },
}

pub enum AppMsg {
    Update {
        provider: Provider,
        source_id: String,
        state: SourceState,
    },
    Error {
        provider: Provider,
        source_id: String,
        error: String,
    },
    Scheduled {
        provider: Provider,
        source_id: String,
        next_at: std::time::Instant,
    },
}

#[derive(Debug)]
pub enum PollCommand {
    ForceRefresh,
    Pause,
    Resume,
}

pub struct SourceSlot {
    pub id: String,
    pub state: SourceState,
    pub poll_tx: mpsc::Sender<PollCommand>,
    pub last_poll_at: Option<std::time::Instant>,
    pub next_poll_at: Option<std::time::Instant>,
}

pub struct Tab {
    pub provider: Provider,
    pub sources: Vec<SourceSlot>,
    pub active: usize,
}

impl Tab {
    pub fn cycle_source(&mut self) {
        if self.sources.len() > 1 {
            self.active = (self.active + 1) % self.sources.len();
        }
    }

    pub fn active_state(&self) -> Option<&SourceState> {
        self.sources.get(self.active).map(|s| &s.state)
    }
}

pub struct AppState {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub last_refresh: Option<std::time::Instant>,
    pub status_message: Option<String>,
    pub theme: Theme,
    /// When true, all sources keep polling in the background and a detected
    /// usage-percentage change automatically switches to that tab.
    pub watch_mode: bool,
}

impl AppState {
    pub fn new(tabs: Vec<Tab>) -> Self {
        Self {
            tabs,
            active_tab: 0,
            last_refresh: None,
            status_message: None,
            theme: Theme::default(),
            watch_mode: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    #[allow(dead_code)]
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab)
    }

    pub fn active_state(&self) -> Option<&SourceState> {
        self.tabs.get(self.active_tab)?.active_state()
    }

    pub fn handle_input(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::Quit)
            }
            KeyCode::Char('r') => {
                self.request_refresh();
                None
            }
            KeyCode::Char('w') => {
                self.toggle_watch_mode();
                None
            }
            KeyCode::Enter => {
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    tab.cycle_source();
                }
                self.status_message = None;
                None
            }
            KeyCode::Tab | KeyCode::Right => {
                self.next_tab();
                None
            }
            KeyCode::BackTab | KeyCode::Left => {
                self.prev_tab();
                None
            }
            KeyCode::Char(c @ '1'..='9') => {
                self.switch_tab((c as usize) - ('1' as usize));
                None
            }
            KeyCode::Down => {
                self.set_theme(self.theme.next());
                None
            }
            KeyCode::Up => {
                self.set_theme(self.theme.prev());
                None
            }
            _ => None,
        }
    }

    pub fn set_theme(&mut self, theme: Theme) {
        if theme != self.theme {
            self.theme = theme;
            self.status_message = Some(format!("theme: {}", theme.label()));
        }
    }

    pub fn switch_tab(&mut self, idx: usize) {
        if idx < self.tabs.len() && idx != self.active_tab {
            self.set_active_tab(idx);
        }
    }

    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            let next = (self.active_tab + 1) % self.tabs.len();
            self.switch_tab(next);
        }
    }

    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            let prev = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
            self.switch_tab(prev);
        }
    }

    fn set_active_tab(&mut self, idx: usize) {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            for slot in &tab.sources {
                if !self.watch_mode {
                    let _ = slot.poll_tx.try_send(PollCommand::Pause);
                }
            }
        }
        self.active_tab = idx;
        if let Some(tab) = self.tabs.get(idx) {
            for slot in &tab.sources {
                let _ = slot.poll_tx.try_send(PollCommand::Resume);
            }
        }
    }

    pub fn toggle_watch_mode(&mut self) {
        self.watch_mode = !self.watch_mode;
        if self.watch_mode {
            for tab in &self.tabs {
                for slot in &tab.sources {
                    let _ = slot.poll_tx.try_send(PollCommand::Resume);
                }
            }
        } else {
            // Re-pause every source except those of the active tab.
            for (i, tab) in self.tabs.iter().enumerate() {
                for slot in &tab.sources {
                    let _ = slot.poll_tx.try_send(if i == self.active_tab {
                        PollCommand::Resume
                    } else {
                        PollCommand::Pause
                    });
                }
            }
        }
    }

    pub fn request_refresh(&mut self) -> RefreshResult {
        if let Some(last) = self.last_refresh {
            let elapsed = last.elapsed();
            if elapsed < cooldown() {
                let remain = (cooldown() - elapsed).as_secs().max(1) as u32;
                self.status_message = Some(format!("Wait {}s to refresh", remain));
                return RefreshResult::CooldownActive {
                    secs_remaining: remain,
                };
            }
        }
        if let Some(tab) = self.tabs.get(self.active_tab) {
            if let Some(slot) = tab.sources.get(tab.active) {
                let _ = slot.poll_tx.try_send(PollCommand::ForceRefresh);
                self.last_refresh = Some(std::time::Instant::now());
                self.status_message = None;
                return RefreshResult::Triggered;
            }
        }
        self.status_message = Some("No source to refresh".into());
        RefreshResult::Triggered
    }

    pub fn apply_update(&mut self, provider: Provider, source_id: &str, state: SourceState) {
        if let Some(tab_idx) = self.tabs.iter().position(|t| t.provider == provider) {
            if let Some(slot) = self.tabs[tab_idx]
                .sources
                .iter_mut()
                .find(|s| s.id == source_id)
            {
                let pct_changed = slot.state.usage_changed(&state);
                slot.state = state;
                if pct_changed && self.watch_mode {
                    self.switch_tab(tab_idx);
                }
            }
        }
        self.status_message = None;
    }

    pub fn apply_error(&mut self, provider: Provider, source_id: &str, error: String) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.provider == provider) {
            if let Some(slot) = tab.sources.iter_mut().find(|s| s.id == source_id) {
                slot.state.set_error(error);
            }
        }
    }

    pub fn apply_scheduled(
        &mut self,
        provider: Provider,
        source_id: &str,
        next_at: std::time::Instant,
    ) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.provider == provider) {
            if let Some(slot) = tab.sources.iter_mut().find(|s| s.id == source_id) {
                slot.last_poll_at = Some(std::time::Instant::now());
                slot.next_poll_at = Some(next_at);
            }
        }
    }

    pub fn active_poll_timing(&self) -> Option<(std::time::Instant, std::time::Instant)> {
        let tab = self.tabs.get(self.active_tab)?;
        let slot = tab.sources.get(tab.active)?;
        Some((slot.last_poll_at?, slot.next_poll_at?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Provider, ProviderState, SourceState};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn make_slot(provider: Provider, id: &str, poll_tx: mpsc::Sender<PollCommand>) -> SourceSlot {
        SourceSlot {
            id: id.to_string(),
            state: SourceState::Quota(ProviderState {
                provider,
                label: provider.label().to_string(),
                windows: vec![],
                last_updated: None,
                last_error: None,
            }),
            poll_tx,
            last_poll_at: None,
            next_poll_at: None,
        }
    }

    fn make_tab(provider: Provider, source_ids: &[&str]) -> Tab {
        let sources = source_ids
            .iter()
            .map(|id| {
                let (tx, _rx) = mpsc::channel(4);
                make_slot(provider, id, tx)
            })
            .collect();
        Tab {
            provider,
            sources,
            active: 0,
        }
    }

    #[test]
    fn cycle_source_wraps_with_multiple_sources() {
        let mut tab = make_tab(Provider::Claude, &["oauth", "api-key"]);
        assert_eq!(tab.active, 0);
        tab.cycle_source();
        assert_eq!(tab.active, 1);
        tab.cycle_source();
        assert_eq!(tab.active, 0);
    }

    #[test]
    fn cycle_source_noop_with_single_source() {
        let mut tab = make_tab(Provider::Zai, &["default"]);
        tab.cycle_source();
        assert_eq!(tab.active, 0);
    }

    #[test]
    fn active_state_reflects_active_index() {
        let mut tab = make_tab(Provider::Claude, &["oauth", "api-key"]);
        tab.active = 1;
        assert_eq!(tab.active_state().unwrap().label(), "Claude");
    }

    #[test]
    fn tab_navigation_wraps() {
        let mut app = AppState::new(vec![
            make_tab(Provider::Claude, &["oauth"]),
            make_tab(Provider::Zai, &["default"]),
            make_tab(Provider::Gemini, &["default"]),
        ]);
        assert_eq!(app.active_tab, 0);
        app.next_tab();
        assert_eq!(app.active_tab, 1);
        app.prev_tab();
        assert_eq!(app.active_tab, 0);
        app.prev_tab();
        assert_eq!(app.active_tab, 2);
        app.switch_tab(1);
        assert_eq!(app.active_tab, 1);
        app.switch_tab(99);
        assert_eq!(app.active_tab, 1);
    }

    #[test]
    fn empty_app_state_navigation_is_noop() {
        let mut app = AppState::new(vec![]);
        assert!(app.is_empty());
        app.next_tab();
        app.prev_tab();
        assert_eq!(app.active_tab, 0);
    }

    #[test]
    fn handle_input_quit_keys() {
        let mut app = AppState::new(vec![make_tab(Provider::Claude, &["oauth"])]);
        assert!(matches!(
            app.handle_input(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(Action::Quit)
        ));
        assert!(matches!(
            app.handle_input(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        ));
        assert!(app
            .handle_input(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE))
            .is_none());
    }

    #[test]
    fn handle_input_tab_switching() {
        let mut app = AppState::new(vec![
            make_tab(Provider::Claude, &["oauth"]),
            make_tab(Provider::Zai, &["default"]),
        ]);
        app.handle_input(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.active_tab, 1);
        app.handle_input(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(app.active_tab, 0);
        app.handle_input(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
        assert_eq!(app.active_tab, 1);
    }

    #[test]
    fn handle_input_enter_cycles_source_and_clears_status() {
        let mut app = AppState::new(vec![make_tab(Provider::Claude, &["oauth", "api-key"])]);
        app.status_message = Some("stale".into());
        app.handle_input(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.tabs[0].active, 1);
        assert!(app.status_message.is_none());
    }

    #[test]
    fn request_refresh_triggers_then_respects_cooldown() {
        let _guard = crate::config::env_var_test_lock().lock().unwrap();
        std::env::set_var("AIBAR_COOLDOWN_SECS", "3600");

        let mut app = AppState::new(vec![make_tab(Provider::Claude, &["oauth"])]);
        assert!(matches!(app.request_refresh(), RefreshResult::Triggered));
        assert!(app.last_refresh.is_some());

        match app.request_refresh() {
            RefreshResult::CooldownActive { secs_remaining } => {
                assert!(secs_remaining > 0);
            }
            RefreshResult::Triggered => panic!("expected cooldown to be active"),
        }
        assert!(app.status_message.as_deref().unwrap().starts_with("Wait"));

        std::env::remove_var("AIBAR_COOLDOWN_SECS");
    }

    #[test]
    fn request_refresh_with_no_tabs_sets_status_message() {
        let mut app = AppState::new(vec![]);
        app.request_refresh();
        assert_eq!(app.status_message.as_deref(), Some("No source to refresh"));
    }

    #[test]
    fn apply_update_sets_matching_slot_state() {
        let mut app = AppState::new(vec![make_tab(Provider::Claude, &["oauth", "api-key"])]);
        let new_state = SourceState::Quota(ProviderState {
            provider: Provider::Claude,
            label: "Claude (Pro)".into(),
            windows: vec![],
            last_updated: None,
            last_error: None,
        });
        app.apply_update(Provider::Claude, "api-key", new_state);
        assert_eq!(app.tabs[0].sources[1].state.label(), "Claude (Pro)");
        assert_eq!(app.tabs[0].sources[0].state.label(), "Claude");
    }

    #[test]
    fn apply_error_sets_error_on_matching_slot() {
        let mut app = AppState::new(vec![make_tab(Provider::Zai, &["default"])]);
        app.apply_error(Provider::Zai, "default", "network down".into());
        assert_eq!(
            app.tabs[0].sources[0].state.last_error(),
            Some("network down")
        );
    }

    #[test]
    fn apply_scheduled_sets_poll_timing() {
        let mut app = AppState::new(vec![make_tab(Provider::Gemini, &["default"])]);
        let next = std::time::Instant::now() + std::time::Duration::from_secs(300);
        app.apply_scheduled(Provider::Gemini, "default", next);
        let slot = &app.tabs[0].sources[0];
        assert!(slot.last_poll_at.is_some());
        assert_eq!(slot.next_poll_at, Some(next));
    }

    #[tokio::test]
    async fn switching_tabs_pauses_old_and_resumes_new_sources() {
        let (tx0, mut rx0) = mpsc::channel(4);
        let (tx1, mut rx1) = mpsc::channel(4);
        let mut app = AppState::new(vec![
            Tab {
                provider: Provider::Claude,
                sources: vec![make_slot(Provider::Claude, "oauth", tx0)],
                active: 0,
            },
            Tab {
                provider: Provider::Zai,
                sources: vec![make_slot(Provider::Zai, "default", tx1)],
                active: 0,
            },
        ]);

        app.switch_tab(1);
        assert!(matches!(rx0.recv().await, Some(PollCommand::Pause)));
        assert!(matches!(rx1.recv().await, Some(PollCommand::Resume)));

        app.switch_tab(0);
        assert!(matches!(rx1.recv().await, Some(PollCommand::Pause)));
        assert!(matches!(rx0.recv().await, Some(PollCommand::Resume)));
    }

    #[tokio::test]
    async fn switching_to_current_tab_sends_no_commands() {
        let (tx, mut rx) = mpsc::channel(4);
        let mut app = AppState::new(vec![Tab {
            provider: Provider::Claude,
            sources: vec![make_slot(Provider::Claude, "oauth", tx)],
            active: 0,
        }]);

        app.switch_tab(0);
        app.next_tab();
        app.prev_tab();
        assert!(rx.try_recv().is_err());
        assert_eq!(app.active_tab, 0);
    }

    #[test]
    fn handle_input_arrow_keys_cycle_theme_with_wrap() {
        let mut app = AppState::new(vec![make_tab(Provider::Claude, &["oauth"])]);
        assert_eq!(app.theme, Theme::Default);

        app.handle_input(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.theme, Theme::Crush);
        app.handle_input(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.theme, Theme::Btop);
        app.handle_input(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.theme, Theme::Default);

        app.handle_input(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.theme, Theme::Btop);
        app.handle_input(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.theme, Theme::Crush);
    }

    #[test]
    fn set_theme_updates_status_message_only_on_change() {
        let mut app = AppState::new(vec![make_tab(Provider::Zai, &["default"])]);
        app.status_message = Some("stale".into());

        app.set_theme(Theme::Btop);
        assert_eq!(app.theme, Theme::Btop);
        assert_eq!(app.status_message.as_deref(), Some("theme: btop"));

        app.set_theme(Theme::Btop);
        assert_eq!(app.status_message.as_deref(), Some("theme: btop"));
    }

    #[test]
    fn apply_update_switches_tab_on_percentage_change_in_watch_mode() {
        let mut app = AppState::new(vec![
            make_tab(Provider::Claude, &["oauth"]),
            make_tab(Provider::Zai, &["default"]),
        ]);

        app.watch_mode = true;

        // Initial update: no previous percentage, so no switch.
        let first = quota_state(Provider::Claude, 10);
        app.apply_update(Provider::Claude, "oauth", first);
        assert_eq!(app.active_tab, 0);

        // Percentage changed → switches to the Claude tab (already 0 here,
        // so switch to the Z.ai tab by updating its percentage instead).
        let first_zai = quota_state(Provider::Zai, 5);
        app.apply_update(Provider::Zai, "default", first_zai);
        assert_eq!(app.active_tab, 0);

        // Update Z.ai again with a changed percentage → switches to tab 1.
        let changed_zai = quota_state(Provider::Zai, 55);
        app.apply_update(Provider::Zai, "default", changed_zai);
        assert_eq!(app.active_tab, 1);
    }

    #[test]
    fn apply_update_does_not_switch_when_watch_mode_off() {
        let mut app = AppState::new(vec![
            make_tab(Provider::Claude, &["oauth"]),
            make_tab(Provider::Zai, &["default"]),
        ]);

        app.apply_update(Provider::Zai, "default", quota_state(Provider::Zai, 5));
        app.apply_update(Provider::Zai, "default", quota_state(Provider::Zai, 55));
        assert_eq!(app.active_tab, 0);
    }

    #[tokio::test]
    async fn toggle_watch_mode_resumes_all_sources() {
        let (tx0, mut rx0) = mpsc::channel(4);
        let (tx1, mut rx1) = mpsc::channel(4);
        let mut app = AppState::new(vec![
            Tab {
                provider: Provider::Claude,
                sources: vec![make_slot(Provider::Claude, "oauth", tx0)],
                active: 0,
            },
            Tab {
                provider: Provider::Zai,
                sources: vec![make_slot(Provider::Zai, "default", tx1)],
                active: 0,
            },
        ]);

        app.toggle_watch_mode();
        assert!(app.watch_mode);
        assert!(matches!(rx1.recv().await, Some(PollCommand::Resume)));
        assert!(matches!(rx0.recv().await, Some(PollCommand::Resume)));

        app.toggle_watch_mode();
        assert!(!app.watch_mode);
        // Active tab (Claude) keeps resuming, Z.ai is paused again.
        assert!(matches!(rx0.recv().await, Some(PollCommand::Resume)));
        assert!(matches!(rx1.recv().await, Some(PollCommand::Pause)));
    }

    fn quota_state(provider: Provider, used: u64) -> SourceState {
        SourceState::Quota(ProviderState {
            provider,
            label: provider.label().to_string(),
            windows: vec![crate::model::LimitWindow::from_values(
                crate::model::WindowKind::FiveHours,
                None,
                used,
                crate::config::NOTIONAL_LIMIT,
                None,
            )],
            last_updated: None,
            last_error: None,
        })
    }
}
