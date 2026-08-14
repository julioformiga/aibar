mod agents;
mod app;
mod cache;
mod config;
mod model;
mod ui;

use crate::agents::{detect_agents, Agent};
use crate::app::{Action, AppMsg, AppState, PollCommand, SourceSlot, Tab};
use crate::cache::{Cache, CachedSource, CachedState};
use crate::config::{http_timeout, max_backoff, poll_interval};
use crate::model::Provider;
use crossterm::{
    event::{Event, EventStream},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use directories::BaseDirs;
use futures::StreamExt;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;

type Tui = Terminal<CrosstermBackend<io::Stdout>>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let mut terminal = setup_terminal()?;
    let result = run_async(&mut terminal).await;
    restore_terminal()?;
    if let Err(e) = result {
        eprintln!("{e:?}");
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn run_async(terminal: &mut Tui) -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cache = Cache::new()?;
    let cached = cache.load().unwrap_or_default();

    let detected = detect_agents();
    if detected.is_empty() {
        let mut app = AppState::new(vec![]);
        loop {
            terminal.draw(|f| ui::draw(f, &app))?;
            let mut events = EventStream::new();
            if let Some(Ok(Event::Key(key))) = events.next().await {
                if matches!(app.handle_input(key), Some(Action::Quit)) {
                    break;
                }
            }
        }
        return Ok(());
    }

    let (app_tx, mut app_rx) = mpsc::channel::<AppMsg>(64);

    let (tabs, initial_tab) = build_tabs(detected, &cached, app_tx.clone());
    let mut app = AppState::new(tabs);
    app.active_tab = initial_tab;

    let mut events = EventStream::new();
    let mut redraw_tick = tokio::time::interval(Duration::from_secs(1));

    loop {
        terminal.draw(|f| ui::draw(f, &app))?;

        tokio::select! {
            maybe_ev = events.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_ev {
                    if matches!(app.handle_input(key), Some(Action::Quit)) {
                        break;
                    }
                    save_cache(&cache, &app);
                }
            }
            maybe_msg = app_rx.recv() => {
                if let Some(msg) = maybe_msg {
                    match msg {
                        AppMsg::Update { provider, source_id, state } => {
                            app.apply_update(provider, &source_id, state);
                            save_cache(&cache, &app);
                        }
                        AppMsg::Error { provider, source_id, error } => {
                            app.apply_error(provider, &source_id, error);
                        }
                        AppMsg::Scheduled { provider, source_id, next_at } => {
                            app.apply_scheduled(provider, &source_id, next_at);
                        }
                    }
                }
            }
            _ = redraw_tick.tick() => {}
        }
    }

    Ok(())
}

fn build_tabs(
    agents: Vec<Box<dyn Agent>>,
    cached: &CachedState,
    app_tx: mpsc::Sender<AppMsg>,
) -> (Vec<Tab>, usize) {
    let mut tabs_map: Vec<(Provider, Vec<Box<dyn Agent>>)> = Vec::new();
    for agent in agents {
        let provider = agent.provider();
        if let Some((_, group)) = tabs_map.iter_mut().find(|(p, _)| *p == provider) {
            group.push(agent);
        } else {
            tabs_map.push((provider, vec![agent]));
        }
    }

    let active_tab_idx = cached.active_tab.min(tabs_map.len().saturating_sub(1));

    let mut tabs = Vec::new();
    for (i, (provider, group)) in tabs_map.into_iter().enumerate() {
        let start_paused = i != active_tab_idx;
        let active = cached
            .active_sources
            .get(provider.label())
            .copied()
            .unwrap_or(0);
        let mut sources = Vec::new();
        for agent in group {
            let source_id = agent.source_id().to_string();
            let cached_state = cached
                .sources
                .iter()
                .find(|cs| cs.provider == provider && cs.source_id == source_id)
                .map(|cs| cs.state.clone());
            let state = cached_state.unwrap_or_else(|| agent.initial_state());

            let (cmd_tx, cmd_rx) = mpsc::channel::<PollCommand>(8);
            tokio::spawn(run_poller(agent, app_tx.clone(), cmd_rx, start_paused));

            sources.push(SourceSlot {
                id: source_id,
                state,
                poll_tx: cmd_tx,
                last_poll_at: None,
                next_poll_at: None,
            });
        }
        let active = active.min(sources.len().saturating_sub(1));
        tabs.push(Tab {
            provider,
            sources,
            active,
        });
    }
    (tabs, active_tab_idx)
}

fn save_cache(cache: &Cache, app: &AppState) {
    let mut active_sources = std::collections::HashMap::new();
    let mut sources = Vec::new();
    for tab in &app.tabs {
        active_sources.insert(tab.provider.label().to_string(), tab.active);
        for slot in &tab.sources {
            sources.push(CachedSource {
                provider: tab.provider,
                source_id: slot.id.clone(),
                state: slot.state.clone(),
            });
        }
    }
    let snapshot = CachedState {
        active_tab: app.active_tab,
        active_sources,
        sources,
    };
    if let Err(e) = cache.save(&snapshot) {
        tracing::warn!(error = %e, "failed to write cache");
    }
}

async fn run_poller(
    agent: Box<dyn Agent>,
    app_tx: mpsc::Sender<AppMsg>,
    mut cmd_rx: mpsc::Receiver<PollCommand>,
    start_paused: bool,
) {
    let provider = agent.provider();
    let source_id = agent.source_id().to_string();
    let mut interval = poll_interval();
    let mut paused = start_paused;

    loop {
        while paused {
            match cmd_rx.recv().await {
                Some(PollCommand::Resume) | Some(PollCommand::ForceRefresh) => paused = false,
                Some(PollCommand::Pause) => {}
                None => return,
            }
        }

        let result = tokio::time::timeout(http_timeout(), agent.fetch()).await;
        let mut wait_for_user = false;
        match result {
            Ok(Ok(state)) => {
                let _ = app_tx
                    .send(AppMsg::Update {
                        provider,
                        source_id: source_id.clone(),
                        state,
                    })
                    .await;
                interval = poll_interval();
            }
            Ok(Err(e)) => {
                let error = e.to_string();
                wait_for_user = agents::gemini::is_login_required(&error);
                let _ = app_tx
                    .send(AppMsg::Error {
                        provider,
                        source_id: source_id.clone(),
                        error,
                    })
                    .await;
                if !wait_for_user {
                    interval = std::cmp::min(interval * 2, max_backoff());
                }
            }
            Err(_) => {
                let _ = app_tx
                    .send(AppMsg::Error {
                        provider,
                        source_id: source_id.clone(),
                        error: "request timed out".into(),
                    })
                    .await;
                interval = std::cmp::min(interval * 2, max_backoff());
            }
        }

        let next_at = std::time::Instant::now()
            + if wait_for_user {
                Duration::from_secs(365 * 24 * 3600)
            } else {
                interval
            };
        let _ = app_tx
            .send(AppMsg::Scheduled {
                provider,
                source_id: source_id.clone(),
                next_at,
            })
            .await;

        if wait_for_user {
            // A login-required error cannot resolve on its own; retrying on
            // a timer would re-spawn agy and reopen the Google login screen.
            // Wait for an explicit refresh (R) instead.
            loop {
                match cmd_rx.recv().await {
                    Some(PollCommand::ForceRefresh) => break,
                    Some(PollCommand::Resume) => paused = false,
                    Some(PollCommand::Pause) => paused = true,
                    None => return,
                }
            }
            continue;
        }

        let deadline = tokio::time::Instant::from_std(next_at);
        let sleep = tokio::time::sleep_until(deadline);
        tokio::pin!(sleep);

        loop {
            if paused {
                match cmd_rx.recv().await {
                    Some(PollCommand::Resume) => paused = false,
                    Some(PollCommand::ForceRefresh) => {
                        paused = false;
                        break;
                    }
                    Some(PollCommand::Pause) => {}
                    None => return,
                }
                continue;
            }
            tokio::select! {
                _ = &mut sleep => break,
                cmd = cmd_rx.recv() => match cmd {
                    Some(PollCommand::ForceRefresh) => break,
                    Some(PollCommand::Pause) => paused = true,
                    Some(PollCommand::Resume) => {}
                    None => return,
                },
            }
        }
    }
}

fn setup_terminal() -> io::Result<Tui> {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        original_hook(info);
    }));
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}

fn init_tracing() {
    let level = std::env::var("AIBAR_LOG").unwrap_or_else(|_| "warn".into());
    let dir = BaseDirs::new().map(|b| b.cache_dir().join("aibar"));
    let builder = tracing_subscriber::fmt().with_env_filter(level);
    if let Some(d) = dir {
        let _ = std::fs::create_dir_all(&d);
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(d.join("aibar.log"))
        {
            builder.with_writer(file).init();
            return;
        }
    }
    builder.with_writer(std::io::stderr).init();
}
