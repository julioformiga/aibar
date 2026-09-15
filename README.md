# aibar

A minimal terminal UI (TUI) that monitors API rate limits for **Claude**,
**Z.ai**, **Gemini**, **OpenAI Codex**, and **Kimi Code** — 5-hour and 7-day
usage windows at a glance — plus the **Hyper** (Charm) Hypercredit balance.

Designed to run in a side pane (e.g. a `tmux` split) and give instant visual
feedback on how close each account is to its limit.

<img width="1215" height="891" alt="AIbar Crush" src="https://github.com/user-attachments/assets/f6bca729-3ec9-4dbe-86e2-6d7c2dfcc055" />

<img width="1171" height="275" alt="AIbar Claude" src="https://github.com/user-attachments/assets/0ced7671-8651-4ca1-ab6a-6d94acea7527" />

## Features

- Auto-detects configured providers/sources from environment variables and
  credential files — no config file needed.
- Supports multiple sources per provider (e.g. Claude OAuth + Claude API
  key), cycled with `Enter`.
- Background async polling, non-blocking UI, with a countdown timer to the
  next poll.
- Watch mode (`w`): keep all sources polling and automatically switch to a
  provider's tab whenever its usage percentage changes.
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
| `HYPER_API_KEY`       | Hyper    | Hyper (Charm) Hypercredit balance |
| `KIMI_API_KEY`        | Kimi Code | Kimi Code source (or `pass KIMI_API_KEY`) |
| `CLAUDE_CONFIG_DIR`   | Claude   | Alternate directory for OAuth credentials |

Fallbacks without an env var:

| Provider | Source  | Fallback                                        |
|----------|---------|--------------------------------------------------|
| Claude   | OAuth   | `~/.claude/.credentials.json`                     |
| Z.ai     | default | `pass Z_AI_API_KEY` (Unix password store)         |
| Kimi Code | default | `pass KIMI_API_KEY` (Unix password store)        |
| Gemini   | default | `~/.gemini/antigravity-cli/log/` (local `agy` server) |
| OpenAI   | codex   | `codex` in `PATH`, signed in with a ChatGPT account |

### Kimi Code

The Kimi Code tab reads `GET https://api.kimi.com/coding/v1/usages` and shows
each rolling rate-limit window the API reports (a 5-hour window in practice)
plus the weekly quota when the plan includes one, with real `used/limit`
counts and reset countdowns.

### OpenAI (Codex)

The OpenAI tab shows the **ChatGPT plan quota that Codex reports** for the
account the [Codex CLI](https://developers.openai.com/codex/cli) is signed in
with. aibar reads it over the Codex app-server (`account/read` and
`account/rateLimits/read`), so it never sends a prompt, starts a turn, or
manages tokens itself — the CLI keeps handling the login.

What it does **not** show: usage of ordinary ChatGPT conversations in the
browser or app, OpenAI API spend, and prepaid credit balances. A plain
`OPENAI_API_KEY` is not used and does not enable this tab.

Window lengths, percentages, and reset times come from Codex as reported;
aibar never assumes fixed limits for a plan. When a window is missing it is
shown as unavailable instead of as zero usage.

Set `AIBAR_CODEX_BIN` to point at a specific `codex` binary. If Codex is not
signed in (or is authenticated with an API key, which carries no plan quota),
the tab shows:

```
codex login required: run `codex login` in another terminal, then press R to retry
```

Auto-retry stops in that case — as with Gemini — so the login flow is not
reopened every poll; press `r` after signing in.

### Keybindings

| Key                | Action                                  |
|---------------------|------------------------------------------|
| `1`–`9`             | Jump directly to tab 1–9                 |
| `Tab` / `→`         | Next tab                                 |
| `Shift+Tab` / `←`   | Previous tab                             |
| `↑` / `↓`           | Cycle theme (default → crush → btop → opencode) |
| `Enter`             | Cycle source (when a provider has more than one) |
| `r`                 | Force refresh (30s cooldown)             |
| `w`                 | Toggle watch mode (auto-switch on change) |
| `q` / `Ctrl+C`      | Quit                                     |

### Mouse

Mouse support is on by default:

- **Click a provider tab** (top bar) to switch to it; clicking the active
  tab cycles its sources (same as `Enter`).
- **Click a footer action** (key + label, e.g. `W Watch (on)`, `R Refresh`,
  `Q Quit`) to trigger it.
- **Scroll on the top bar** to move to the previous/next tab.

Set `AIBAR_NO_MOUSE=1` to disable mouse capture (restores the terminal's
native click-drag text selection).

### Themes

Four built-in themes, cycled with `↑`/`↓` and persisted in the local cache:

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
- **opencode** — based on OpenCode's default theme (dark variant): mostly
  grays — near-black `#0A0A0A` panel background, subtle gray rounded border
  (`#484848`), white bold title and active tab, muted `#808080` text — with
  braille-dot bars (`[⣿⣀]`) and percentages in OpenCode's secondary blue
  (`#5C9CF5`), time/token indicators in white, and a blue→purple timer
  gradient (`#5C9CF5`→`#9D7CD8`).

### Optional configuration

| Variable              | Default | Description                     |
|------------------------|---------|----------------------------------|
| `AIBAR_POLL_SECS`      | `120`   | Background polling interval (seconds) |
| `AIBAR_COOLDOWN_SECS`  | `30`    | Cooldown for manual refresh (`r`) |
| `AIBAR_HYPER_PLAN`     | auto    | Force the Hyper plan: `free` or `monthly` |
| `AIBAR_CODEX_BIN`      | `codex` in `PATH` | Path to the Codex CLI binary |
| `AIBAR_NO_MOUSE`       | unset   | Set to `1` to disable mouse capture |
| `AIBAR_LOG`            | `warn`  | `tracing` log level              |

### Hyper plan and daily reset

Hyper's `/v1/credits` endpoint reports only the raw balance — not the team's
plan — but the percentage depends on it: the free plan grants 100
Hypercredits/month while the subscription grants 250/day. aibar resolves
the plan as: `AIBAR_HYPER_PLAN` override, then a sticky detection (a balance
above 100 can only come from the subscription, and the detection survives
the end of the day when the balance drops below 100), then the balance
heuristic. On the monthly plan the line also shows a countdown to the next
daily refresh: since the API does not expose the reset time, it is anchored
whenever the balance is observed rising between two polls (`--` until the
first rise is seen). If you switch plans, set `AIBAR_HYPER_PLAN`
explicitly (the sticky detection persists in the cache).

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
