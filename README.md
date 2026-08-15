# aibar

A minimal terminal UI (TUI) that monitors API rate limits for **Claude**,
**Z.ai**, and **Gemini** — 5-hour and 7-day usage windows at a glance.

Designed to run in a side pane (e.g. a `tmux` split) and give instant visual
feedback on how close each account is to its limit.

<img width="857" height="93" alt="image" src="https://github.com/user-attachments/assets/54f36b40-38a0-4085-ae99-1a43e40d74ca" />

## Features

- Auto-detects configured providers/sources from environment variables and
  credential files — no config file needed.
- Supports multiple sources per provider (e.g. Claude OAuth + Claude API
  key), cycled with `Enter`.
- Background async polling, non-blocking UI, with a countdown timer to the
  next poll.
- Local cache (`~/.cache/aibar/state.json`) so the last known state survives
  restarts.

## Installation

Requires a recent Rust toolchain ([rustup](https://rustup.rs)).

```sh
git clone https://github.com/julioformiga/aibar.git
cd aibar
cargo install --path .
```

Or build without installing:

```sh
cargo build --release
./target/release/aibar
```

## Usage

Set at least one of the following environment variables, then run `aibar`:

| Variable             | Provider | Enables                          |
|-----------------------|----------|-----------------------------------|
| `ANTHROPIC_API_KEY`   | Claude   | Claude (API) source               |
| `ZAI_API_KEY`         | Z.ai     | Z.ai source (or `pass Z_AI_API_KEY`) |
| `GEMINI_API_KEY`      | Gemini   | Gemini source (optional if Antigravity/`agy` is installed) |
| `CLAUDE_CONFIG_DIR`   | Claude   | Alternate directory for OAuth credentials |

Fallbacks without an env var:

| Provider | Source  | Fallback                                        |
|----------|---------|--------------------------------------------------|
| Claude   | OAuth   | `~/.claude/.credentials.json`                     |
| Z.ai     | default | `pass Z_AI_API_KEY` (Unix password store)         |
| Gemini   | default | `~/.gemini/antigravity-cli/log/` (local `agy` server) |

### Keybindings

| Key                | Action                                  |
|---------------------|------------------------------------------|
| `1`–`9`             | Jump directly to tab 1–9                 |
| `Tab` / `→`         | Next tab                                 |
| `Shift+Tab` / `←`   | Previous tab                             |
| `↑` / `↓`           | Cycle theme (default → crush → btop)     |
| `Enter`             | Cycle source (when a provider has more than one) |
| `r`                 | Force refresh (30s cooldown)             |
| `q` / `Ctrl+C`      | Quit                                     |

### Themes

Three built-in themes, cycled with `↑`/`↓` and persisted in the local cache:

- **default** — classic aibar: rounded `[`/`]` blocks bars, green/yellow/red
  by usage threshold.
- **crush** — Charm's Charmtone Pantera palette (as used by the Crush agent):
  rounded lilac border (`#8B75FF`, like Crush's input box) over a dark
  `#1F1C23` background painted across the whole panel, bright white title,
  magenta active tab, `(━┄)` bars with a Hazy→Dolly gradient
  (`#8B75FF`→`#FF60FF`), mint/yellow/pink severity colors, and subtle text
  kept at `#858392` for legibility on dark backgrounds.
- **btop** — inspired by btop's default theme: colored box border, red
  selection-highlighted tab, green→yellow→red CPU-style gradient bars with a
  solid track.

### Optional configuration

| Variable              | Default | Description                     |
|------------------------|---------|----------------------------------|
| `AIBAR_POLL_SECS`      | `300`   | Background polling interval (seconds) |
| `AIBAR_COOLDOWN_SECS`  | `30`    | Cooldown for manual refresh (`r`) |
| `AIBAR_LOG`            | `warn`  | `tracing` log level              |

Logs are written to `~/.cache/aibar/aibar.log`.

## Troubleshooting

### Gemini asks for Google login

The Gemini quota comes from the local Antigravity (`agy`) language server,
which requires Google authentication — there is no way to fetch it
unauthenticated. If the quota poll happens before `agy` has valid
credentials, aibar detects it, stops auto-retrying (so the Google login
screen is not reopened on every poll), and shows:

```
agy login required: run `agy` in another terminal to log in, then press R to retry
```

Run `agy` once in another terminal to authenticate silently (cached
credentials), then press `r` in aibar to retry.

## Development

```sh
cargo build
cargo test
cargo clippy --all-targets
cargo fmt
```

See [SPEC.md](SPEC.md) for the full technical specification (data model,
UI layout, per-provider agent details, and architecture).
