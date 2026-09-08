# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`aibar` is a Rust TUI (ratatui + crossterm + tokio) that polls AI provider APIs
(Claude, Z.ai, Gemini, Hyper, OpenAI Codex) for rate-limit / credit usage and
displays them as tabs with progress bars. Runs in a terminal pane (e.g. a tmux
split).

Full technical spec (data model, per-provider agent details, polling state
machine, UI layout) lives in `SPEC.md` (Portuguese) — read it before making
non-trivial changes; keep it in sync with behavior changes. `README.md` has
the user-facing feature/config/keybinding reference.

## Commands

```sh
cargo build
cargo test                       # all tests are inline `#[cfg(test)] mod tests` per file
cargo test <substring>           # run a single test / module by name filter
cargo clippy --all-targets -- -D warnings
cargo fmt                        # cargo fmt --check in CI
cargo build --release            # opt-level 3, lto, strip (see Cargo.toml)
```

CI (`.github/workflows/ci.yml`) runs fmt check, clippy (deny warnings), build,
and test on every push/PR to `main` — match that locally before considering
work done.

## Architecture

### Agent trait — how a provider gets added

Every data source implements the async `Agent` trait (`src/agents/mod.rs`):
`provider()`, `source_id()`, `initial_state()`, `async fetch()`. A provider
can expose multiple agents/sources (e.g. Claude has both an OAuth agent and
an API-key agent, cycled with `Enter`); `detect_agents()` in `agents/mod.rs`
is the single place that probes env vars / credential files and constructs
whichever agents are actually configured. Each provider's detection lives in
its own `agents/<provider>.rs` (`claude.rs`, `zai.rs`, `gemini.rs`,
`hyper.rs`, `openai.rs`), following the pattern of `from_env()` /
`detect_all()` constructors returning `Option<Self>` / `Vec<Self>` when not
configured, plus free functions for response parsing that are unit-tested
directly (parsing logic is kept out of `fetch()` bodies specifically so it can
be tested without an HTTP layer). `agents/gemini.rs` and `agents/openai.rs`
additionally expose `is_login_required(&str)`, used by `main.rs`'s poller to
detect the un-recoverable "needs interactive login" error and stop
auto-retrying until the user presses `r`.

### Data model — three shapes of "usage" (`src/model.rs`)

`SourceState` is an enum over three ways a source reports usage:
- `Quota(ProviderState)` — one or more `LimitWindow`s (5h / 7d, used/limit),
  the normal case for Claude OAuth, Z.ai, Gemini, and OpenAI Codex (whose
  window lengths come from the API and may be arbitrary durations —
  `WindowKind::Minutes(u32)` / `Unknown` — not just 5h/7d).
- `Ceiling(CeilingReport)` — RPM/TPM ceilings with no percentage (Claude API
  key rate_limits endpoint).
- `Credits(CreditsState)` — a raw balance (Hyper). The plan backing the
  bar (free 100/mo vs monthly 250/day) is not exposed by the API, so
  `CreditsState::resolved_plan` resolves it from the `AIBAR_HYPER_PLAN`
  override, a sticky detection in `apply_update` (balance > 100 ⇒ monthly,
  persisted via cache), then the balance heuristic; the daily-reset
  countdown is anchored whenever the balance rises between polls.

`SourceState::usage_changed(&self, &other)` is the trigger for watch-mode
auto-switching (see below); it compares *every* window, not just the max, and
treats "no prior data" as no change so the first poll never fires a switch.
`Ceiling` never reports a change. When adding a new provider/source kind,
extend this match rather than adding a parallel comparison path.

`LimitWindow::from_fraction` (percentage-only APIs) vs `from_values`
(used/limit APIs) both normalize to `NOTIONAL_LIMIT` (`config.rs`) so bars
render consistently regardless of what the underlying API actually reports.

### Async runtime shape (`src/main.rs`)

One `tokio::spawn`ed poller task per source (`run_poller`), each owning a
`mpsc::Receiver<PollCommand>` (`ForceRefresh` / `Pause` / `Resume`) and
reporting back over a single shared `mpsc::Sender<AppMsg>`
(`Update` / `Error` / `Scheduled`). The main loop is a `tokio::select!` over
input events, `app_rx`, and a 1s redraw tick — it never awaits inside a poller
directly; all cross-task communication is via the two channels. Pollers
self-manage backoff (doubling up to `MAX_BACKOFF_SECS` on error, resetting on
success) and pause/resume state; `AppState` in `app.rs` only sends commands
and folds incoming messages into UI state, it does not own timing.

Only sources belonging to the active tab poll by default; switching tabs
pauses the old tab's sources and resumes the new one's — *unless* watch mode
is on (default on, `w` to toggle), in which case all sources poll
continuously and `AppState::apply_update` auto-switches to whichever tab just
had a `usage_changed` update.

Local state (cache) round-trips through `Cache`/`CachedState`
(`src/cache.rs`, `~/.cache/aibar/state.json`, atomic write via temp file +
rename) — active tab, active source per provider, last known `SourceState`
per source, and theme, so a restart shows last-known values immediately while
pollers refresh in the background.

### Config knobs (`src/config.rs`)

All tunables are read from `AIBAR_*` env vars with compiled-in defaults
(`poll_interval`, `cooldown`, `http_timeout`, `max_backoff`) — add new ones
here rather than inlining `env::var` calls elsewhere. Tests that mutate these
process-wide env vars must take `env_var_test_lock()` first (`cargo test`
runs tests in the same process across threads).

### UI (`src/ui.rs`, `src/theme.rs`)

`ui::draw` is a pure function of `&AppState`; it does no I/O and holds no
state of its own. Three built-in `Theme`s (`Default`/`Crush`/`Btop`) are
cycled with `↑`/`↓` and persisted in the cache; a theme defines colors/border
style only, not layout.

## Notes

- TLS: `reqwest` uses `rustls-no-provider` + `rustls` with the `ring` crypto
  provider installed manually in `main.rs`
  (`rustls::crypto::ring::default_provider().install_default()`) — this
  avoids `aws-lc-sys`, which requires a C toolchain. Don't add a dependency
  that pulls in `aws-lc-sys` without checking this still holds.
- HTTP-layer tests use `mockito` (dev-dependency); prefer testing parsing
  logic as free functions over standing up a mock server when possible.
- Provider-facing strings mix languages: SPEC.md and some inline comments are
  in Portuguese, README/code identifiers are in English — match the existing
  convention in whichever file you're editing.
