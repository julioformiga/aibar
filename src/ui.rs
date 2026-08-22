use crate::app::AppState;
use crate::config::HYPER_FREE_CREDITS;
use crate::model::{CreditsState, LimitScope, LimitWindow, Provider, SourceState, WindowKind};
use crate::theme::Palette;
use chrono::{DateTime, Utc};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use ratatui::Frame;
use std::time::Instant;

const TIME_W: usize = 9;
const SUFFIX_W: usize = 13;
const WARN: &str = "\u{26a0}";
const TIMER_BAR_W: usize = 20;
const TIMER_FILLED: char = '\u{2501}';
const TIMER_EMPTY: char = '\u{2504}';

pub fn draw(f: &mut Frame, app: &AppState) {
    let palette = app.theme.palette();
    let area = f.area();
    let title = breadcrumb_title(app, &palette);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(palette.border)
        .title(title);
    if let Some(c) = palette.bg {
        block = block.style(Style::default().bg(c));
    }
    let border_style = match (palette.border_color, palette.bg) {
        (Some(fg), Some(bg)) => Style::default().fg(fg).bg(bg),
        (Some(fg), None) => Style::default().fg(fg),
        (None, Some(bg)) => Style::default().bg(bg),
        (None, None) => Style::default(),
    };
    block = block.border_style(border_style);

    if !app.is_empty() {
        if let Some(t) = build_timer_line(app, &palette) {
            block = block.title(t.right_aligned());
        }
        let has_multi = app
            .tabs
            .get(app.active_tab)
            .map(|t| t.sources.len() > 1)
            .unwrap_or(false);
        block = block
            .title_bottom(build_status_line(app, &palette))
            .title_bottom(build_hint_line(has_multi, &palette).right_aligned());
    }

    let inner = block.inner(area);
    block.render(area, f.buffer_mut());

    if app.is_empty() {
        draw_welcome(f, inner, &palette);
        return;
    }

    draw_content(f, app, inner, &palette);
}

fn breadcrumb_title(app: &AppState, p: &Palette) -> Line<'static> {
    let mut spans = vec![Span::styled(" AIBar ", p.title)];
    for (i, tab) in app.tabs.iter().enumerate() {
        spans.push(tab_separator(p));
        spans.push(Span::raw(" "));
        let label = tab
            .active_state()
            .map(|s| s.label().to_string())
            .unwrap_or_else(|| tab.provider.label().to_string());
        if i == app.active_tab {
            spans.push(Span::styled(label, p.tab_active));
        } else {
            spans.push(Span::styled(label, p.tab_inactive));
        }
        spans.push(Span::raw(" "));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn tab_separator(p: &Palette) -> Span<'static> {
    match p.tab_separator {
        Some(c) => Span::styled("\u{2502}", Style::default().fg(c)),
        None => Span::raw("\u{2502}"),
    }
}

fn build_status_line(app: &AppState, p: &Palette) -> Line<'static> {
    let active = app.active_state();
    if let Some(err) = active.and_then(|s| s.last_error()) {
        Line::from(Span::styled(
            format!(" {}  {} ", WARN, err),
            Style::default().fg(p.error),
        ))
    } else if let Some(msg) = &app.status_message {
        Line::from(Span::styled(
            format!(" {} ", msg),
            Style::default().fg(p.status),
        ))
    } else {
        Line::from("")
    }
}

fn hint_text(p: &Palette, text: &str) -> Span<'static> {
    match p.hint_text {
        Some(c) => Span::styled(text.to_string(), Style::default().fg(c)),
        None => Span::raw(text.to_string()),
    }
}

fn build_hint_line(has_multi: bool, p: &Palette) -> Line<'static> {
    let bold = p.hint_key;
    let sep = "  ";

    let mut spans = Vec::new();
    if has_multi {
        spans.extend_from_slice(&[
            Span::styled("E", bold),
            hint_text(p, "nter Source"),
            Span::raw(sep),
        ]);
    }
    spans.extend_from_slice(&[
        Span::styled("\u{2191}\u{2193}", bold),
        hint_text(p, " Theme"),
        Span::raw(sep),
        Span::styled("R", bold),
        hint_text(p, "efresh"),
        Span::raw(sep),
        Span::styled("Q", bold),
        hint_text(p, "uit"),
    ]);

    Line::from(spans)
}

fn build_timer_line(app: &AppState, p: &Palette) -> Option<Line<'static>> {
    let (started, next) = app.active_poll_timing()?;
    let now = Instant::now();

    let total = next.saturating_duration_since(started);
    if total.is_zero() {
        return None;
    }
    let elapsed = now.saturating_duration_since(started);
    let frac = (elapsed.as_secs_f64() / total.as_secs_f64()).clamp(0.0, 1.0);

    let filled = ((TIMER_BAR_W as f64) * frac).round() as usize;
    let filled = filled.min(TIMER_BAR_W);
    let empty = TIMER_BAR_W - filled;

    let mut spans = Vec::new();
    match p.timer_filled {
        Some(fill) => {
            for i in 0..filled {
                let pos = i as f32 / TIMER_BAR_W as f32;
                spans.push(Span::styled(
                    TIMER_FILLED.to_string(),
                    Style::default().fg(fill(pos)),
                ));
            }
        }
        None => spans.push(Span::raw(repeat_char(TIMER_FILLED, filled))),
    }
    if empty > 0 {
        let s = repeat_char(TIMER_EMPTY, empty);
        spans.push(match p.timer_empty {
            Some(c) => Span::styled(s, Style::default().fg(c)),
            None => Span::raw(s),
        });
    }

    Some(Line::from(spans))
}

fn draw_content(f: &mut Frame, app: &AppState, area: Rect, p: &Palette) {
    let Some(state) = app.active_state() else {
        return;
    };

    let width = area.width as usize;
    let lines = match state {
        SourceState::Quota(ps) => draw_quota_lines(ps, width, p),
        SourceState::Ceiling(cr) => draw_ceiling_lines(cr, width, p),
        SourceState::Credits(cs) => draw_credits_lines(cs, width, p),
    };

    Paragraph::new(lines).render(area, f.buffer_mut());
}

fn draw_quota_lines(
    ps: &crate::model::ProviderState,
    width: usize,
    p: &Palette,
) -> Vec<Line<'static>> {
    let has_data = !ps.windows.is_empty();
    let cached = ps.last_error.is_some() && has_data;

    let labels: Vec<String> = if has_data {
        ps.windows
            .iter()
            .map(|w| scope_label(w).to_string())
            .collect()
    } else {
        placeholder_labels(ps.provider)
    };
    let label_w = labels.iter().map(|l| l.len()).max().unwrap_or(0).max(3);

    let mut lines: Vec<Line> = Vec::new();
    if has_data {
        for (i, w) in ps.windows.iter().enumerate() {
            let show_warn = cached && i == 0;
            lines.push(build_bar_line(w, label_w, show_warn, cached, width, p));
        }
    } else {
        let kinds = placeholder_kinds(ps.provider);
        for (label, kind) in labels.iter().zip(kinds.iter()) {
            lines.push(build_loading_line(label, *kind, label_w, width, p));
        }
    }
    lines
}

fn draw_ceiling_lines(
    cr: &crate::model::CeilingReport,
    _width: usize,
    p: &Palette,
) -> Vec<Line<'static>> {
    if cr.ceilings.is_empty() {
        let mut spans = Vec::new();
        if cr.last_error.is_some() {
            spans.push(Span::styled(
                format!(" {}  ", WARN),
                Style::default().fg(p.warn),
            ));
        }
        spans.push(Span::styled(
            "Loading rate limits\u{2026}",
            Style::default().fg(p.loading),
        ));
        return vec![Line::from(spans)];
    }

    let max_group = cr
        .ceilings
        .iter()
        .map(|c| c.group.len())
        .max()
        .unwrap_or(8)
        .max(8);

    let mut lines: Vec<Line> = Vec::new();
    for c in &cr.ceilings {
        let mut spans = Vec::new();
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{:<w$}", c.group, w = max_group),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{:>4} RPM", c.rpm),
            Style::default().fg(p.ceiling_rpm),
        ));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{:>6} in", fmt_count(c.in_tpm)),
            Style::default().fg(p.ceiling_in),
        ));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{:>6} out", fmt_count(c.out_tpm)),
            Style::default().fg(p.ceiling_out),
        ));
        lines.push(Line::from(spans));
    }
    lines
}

fn build_credits_line(cs: &CreditsState, width: usize, p: &Palette) -> Line<'static> {
    let Some(balance) = cs.balance else {
        return Line::from(Span::styled(
            "  Loading credits\u{2026}",
            Style::default().fg(p.loading),
        ));
    };

    let cached = cs.last_error.is_some();
    let allowance = HYPER_FREE_CREDITS.max(balance);
    let spent = allowance - balance;
    let pct = if allowance > 0.0 {
        ((spent / allowance) * 100.0) as f32
    } else {
        0.0
    };

    let label = "Credits";
    let label_w = label.len();
    let pct_str = format!("{:>3.0}%", pct);
    let suffix = if cached {
        "(cached)".to_string()
    } else {
        format!("bal {}", fmt_balance(balance))
    };

    let prefix_len = 2 + label_w + 1;
    let suffix_len = 1 + pct_str.len() + 1 + SUFFIX_W;
    let bar_width = width.saturating_sub(prefix_len + suffix_len + 2).max(10);

    let filled = ((bar_width as f32) * (pct / 100.0)).round() as usize;
    let filled = filled.min(bar_width);
    let empty = bar_width - filled;

    let mut spans = Vec::new();
    if cached {
        spans.push(Span::styled(
            format!("{} ", WARN),
            Style::default().fg(p.warn),
        ));
    } else {
        spans.push(Span::raw("  "));
    }
    spans.push(Span::raw(label.to_string()));
    spans.push(Span::raw(" "));
    spans.push(Span::raw(p.bar_open));
    let denom = bar_width.saturating_sub(1).max(1) as f32;
    for i in 0..filled {
        let pos = if bar_width > 1 { i as f32 / denom } else { 0.0 };
        let color = (p.bar_fill)(pct, pos);
        spans.push(Span::styled(
            p.filled_char.to_string(),
            Style::default().fg(color),
        ));
    }
    if empty > 0 {
        spans.push(Span::styled(
            repeat_char(p.empty_char, empty),
            p.empty_style,
        ));
    }
    spans.push(Span::raw(p.bar_close));
    spans.push(Span::styled(
        pct_str,
        Style::default().fg((p.pct_color)(pct)),
    ));
    spans.push(Span::raw(format!(" {:<width$}", suffix, width = SUFFIX_W)));

    Line::from(spans)
}

fn draw_credits_lines(cs: &CreditsState, width: usize, p: &Palette) -> Vec<Line<'static>> {
    vec![build_credits_line(cs, width, p)]
}

fn build_bar_line(
    window: &LimitWindow,
    label_w: usize,
    show_warn: bool,
    cached: bool,
    width: usize,
    p: &Palette,
) -> Line<'static> {
    let pct = window.percentage();
    let label = scope_label(window);
    let countdown = reset_countdown(window.reset_at);
    let kind_str = window_kind_str(window.kind);
    let pct_str = format!("{:>3.0}%", pct);
    let suffix = if cached {
        "(cached)".to_string()
    } else {
        format!("{}/{}", fmt_count(window.used), fmt_count(window.limit))
    };

    let prefix_len = 2 + label_w + 1 + TIME_W + 2;
    let suffix_len = 1 + pct_str.len() + 1 + SUFFIX_W;
    let bar_width = width.saturating_sub(prefix_len + suffix_len + 2).max(10);

    let filled = ((bar_width as f32) * (pct / 100.0)).round() as usize;
    let filled = filled.min(bar_width);
    let empty = bar_width - filled;

    let time_part = format!("{:>width$}", countdown, width = TIME_W - 3);
    let kind_part = format!("/{}", kind_str);

    let mut spans = Vec::new();

    if show_warn {
        spans.push(Span::styled(
            format!("{} ", WARN),
            Style::default().fg(p.warn),
        ));
    } else {
        spans.push(Span::raw("  "));
    }

    spans.push(Span::raw(format!("{:<width$}", label, width = label_w)));
    spans.push(Span::raw(" "));
    spans.push(Span::raw(time_part));
    spans.push(Span::styled(kind_part, Style::default().fg(p.kind)));
    spans.push(Span::raw(p.bar_open));
    let denom = bar_width.saturating_sub(1).max(1) as f32;
    for i in 0..filled {
        let pos = if bar_width > 1 { i as f32 / denom } else { 0.0 };
        let color = (p.bar_fill)(pct, pos);
        spans.push(Span::styled(
            p.filled_char.to_string(),
            Style::default().fg(color),
        ));
    }
    if empty > 0 {
        spans.push(Span::styled(
            repeat_char(p.empty_char, empty),
            p.empty_style,
        ));
    }
    spans.push(Span::raw(p.bar_close));
    spans.push(Span::styled(
        pct_str,
        Style::default().fg((p.pct_color)(pct)),
    ));
    spans.push(Span::raw(format!(" {:<width$}", suffix, width = SUFFIX_W)));

    Line::from(spans)
}

fn build_loading_line(
    label: &str,
    kind: WindowKind,
    label_w: usize,
    width: usize,
    p: &Palette,
) -> Line<'static> {
    let prefix_len = 2 + label_w + 1 + TIME_W + 2;
    let pct_str = "  \u{2014}%";
    let suffix_len = 1 + pct_str.len() + 1 + SUFFIX_W;
    let bar_width = width.saturating_sub(prefix_len + suffix_len + 2).max(10);

    let kind_str = window_kind_str(kind);

    let mut spans = Vec::new();
    spans.push(Span::raw("  "));
    spans.push(Span::raw(format!("{:<width$}", label, width = label_w)));
    spans.push(Span::raw(format!(" {:>width$}", "--", width = TIME_W - 3)));
    spans.push(Span::styled(
        format!("/{}", kind_str),
        Style::default().fg(p.kind),
    ));
    spans.push(Span::raw(p.bar_open));
    spans.push(Span::styled(
        repeat_char(p.empty_char, bar_width),
        p.empty_style,
    ));
    spans.push(Span::raw(p.bar_close));
    spans.push(Span::styled("\u{2014}", Style::default().fg(p.loading)));

    Line::from(spans)
}

fn draw_welcome(f: &mut Frame, area: Rect, p: &Palette) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled("No provider detected.", p.welcome_title)),
        Line::from(""),
        Line::from("Set at least one of these environment variables:"),
        Line::from(""),
        Line::from("  export ANTHROPIC_API_KEY=\"...\"   # Claude (API)"),
        Line::from("  export ZAI_API_KEY=\"...\"         # Z.ai"),
        Line::from("  export GEMINI_API_KEY=\"...\"      # Gemini"),
        Line::from("  export HYPER_API_KEY=\"...\"       # Hyper (Charm)"),
        Line::from(""),
        Line::from("Fallbacks: ~/.claude/.credentials.json,"),
        Line::from("  pass Z_AI_API_KEY, Antigravity (agy)"),
        Line::from(""),
        Line::from("[q] Quit"),
    ];
    Paragraph::new(lines)
        .alignment(Alignment::Center)
        .render(area, f.buffer_mut());
}

fn scope_label(window: &LimitWindow) -> &str {
    match window.scope {
        Some(LimitScope::Standard) => "Google",
        Some(LimitScope::ThirdParty) => "Partner",
        None => "",
    }
}

fn window_kind_str(kind: WindowKind) -> &'static str {
    match kind {
        WindowKind::FiveHours => "5h",
        WindowKind::SevenDays => "7d",
    }
}

fn placeholder_labels(provider: Provider) -> Vec<String> {
    match provider {
        Provider::Claude | Provider::Zai => vec!["".to_string(), "".to_string()],
        Provider::Hyper => vec![],
        Provider::Gemini => vec![
            "Google".into(),
            "Google".into(),
            "Partner".into(),
            "Partner".into(),
        ],
    }
}

fn placeholder_kinds(provider: Provider) -> Vec<WindowKind> {
    match provider {
        Provider::Claude | Provider::Zai => vec![WindowKind::FiveHours, WindowKind::SevenDays],
        Provider::Hyper => vec![],
        Provider::Gemini => vec![
            WindowKind::FiveHours,
            WindowKind::SevenDays,
            WindowKind::FiveHours,
            WindowKind::SevenDays,
        ],
    }
}

fn reset_countdown(reset_at: Option<DateTime<Utc>>) -> String {
    match reset_at {
        None => "--".to_string(),
        Some(t) => {
            let now = Utc::now();
            if t <= now {
                return "now".to_string();
            }
            let secs = (t - now).num_seconds().max(0) as u64;
            let days = secs / 86_400;
            let hours = (secs % 86_400) / 3_600;
            let mins = (secs % 3_600) / 60;
            if days > 0 {
                format!("{}d{:02}h", days, hours)
            } else {
                format!("{}h{:02}m", hours, mins)
            }
        }
    }
}

fn fmt_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1000 {
        format!("{:.0}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

fn fmt_balance(b: f64) -> String {
    if (b - b.round()).abs() < f64::EPSILON {
        format!("{}", b.round() as u64)
    } else {
        format!("{:.1}", b)
    }
}

fn repeat_char(ch: char, n: usize) -> String {
    std::iter::repeat_n(ch, n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use chrono::Duration as ChronoDuration;

    #[test]
    fn fmt_count_formats_thousands_and_millions() {
        assert_eq!(fmt_count(0), "0");
        assert_eq!(fmt_count(999), "999");
        assert_eq!(fmt_count(1000), "1k");
        assert_eq!(fmt_count(1100), "1k");
        assert_eq!(fmt_count(1_500_000), "1.5M");
    }

    #[test]
    fn reset_countdown_none_is_dashes() {
        assert_eq!(reset_countdown(None), "--");
    }

    #[test]
    fn reset_countdown_past_time_is_now() {
        let past = Utc::now() - ChronoDuration::seconds(5);
        assert_eq!(reset_countdown(Some(past)), "now");
    }

    #[test]
    fn reset_countdown_formats_hours_and_minutes() {
        let target = Utc::now() + ChronoDuration::seconds(90);
        assert_eq!(reset_countdown(Some(target)), "0h01m");
    }

    #[test]
    fn reset_countdown_formats_days_and_hours() {
        let target = Utc::now() + ChronoDuration::seconds(91_800);
        assert_eq!(reset_countdown(Some(target)), "1d01h");
    }

    #[test]
    fn scope_label_maps_known_scopes() {
        let w = |scope| LimitWindow {
            kind: WindowKind::FiveHours,
            scope,
            used: 0,
            limit: 1000,
            reset_at: None,
        };
        assert_eq!(scope_label(&w(Some(LimitScope::Standard))), "Google");
        assert_eq!(scope_label(&w(Some(LimitScope::ThirdParty))), "Partner");
        assert_eq!(scope_label(&w(None)), "");
    }

    #[test]
    fn window_kind_str_matches_spec_format() {
        assert_eq!(window_kind_str(WindowKind::FiveHours), "5h");
        assert_eq!(window_kind_str(WindowKind::SevenDays), "7d");
    }

    fn credits_state(balance: Option<f64>, last_error: Option<String>) -> CreditsState {
        CreditsState {
            label: "Hyper".into(),
            balance,
            last_updated: None,
            last_error,
        }
    }

    #[test]
    fn fmt_balance_drops_trailing_zero_and_keeps_one_decimal() {
        assert_eq!(fmt_balance(100.0), "100");
        assert_eq!(fmt_balance(42.5), "42.5");
    }

    #[test]
    fn credits_line_shows_spent_pct_and_balance() {
        let p = Theme::Default.palette();
        let line = build_credits_line(&credits_state(Some(30.0), None), 80, &p);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains(" 70%"), "text was: {text}");
        assert!(text.contains("bal 30"), "text was: {text}");
    }

    #[test]
    fn credits_line_above_free_allowance_is_zero_spent() {
        let p = Theme::Default.palette();
        let line = build_credits_line(&credits_state(Some(250.0), None), 80, &p);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("  0%"), "text was: {text}");
        assert!(text.contains("bal 250"), "text was: {text}");
    }

    #[test]
    fn credits_line_formats_fractional_balance() {
        let p = Theme::Default.palette();
        let line = build_credits_line(&credits_state(Some(42.5), None), 80, &p);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains(" 58%"), "text was: {text}");
        assert!(text.contains("bal 42.5"), "text was: {text}");
    }

    #[test]
    fn credits_line_cached_shows_warn_and_cached_suffix() {
        let p = Theme::Default.palette();
        let line = build_credits_line(&credits_state(Some(10.0), Some("boom".into())), 80, &p);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains(WARN), "text was: {text}");
        assert!(text.contains("(cached)"), "text was: {text}");
    }

    #[test]
    fn credits_line_without_balance_shows_loading() {
        let p = Theme::Default.palette();
        let line = build_credits_line(&credits_state(None, None), 80, &p);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("Loading credits"), "text was: {text}");
    }

    #[test]
    fn bar_line_uses_palette_chars_and_gradient() {
        let w = LimitWindow {
            kind: WindowKind::FiveHours,
            scope: None,
            used: 500,
            limit: 1000,
            reset_at: None,
        };
        let btop = Theme::Btop.palette();
        let line = build_bar_line(&w, 7, false, false, 80, &btop);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains(" 50%"), "text was: {text}");
        assert!(!text.contains('['), "btop bars have no brackets: {text}");
        let filled_spans: Vec<&Span> = line
            .spans
            .iter()
            .filter(|s| s.content.contains('\u{2588}'))
            .collect();
        assert!(!filled_spans.is_empty());
        let first_color = filled_spans[0].style.fg;
        let last_color = filled_spans[filled_spans.len() - 1].style.fg;
        assert_ne!(first_color, last_color);
    }

    #[test]
    fn bar_line_default_theme_keeps_brackets() {
        let w = LimitWindow {
            kind: WindowKind::SevenDays,
            scope: None,
            used: 40,
            limit: 1000,
            reset_at: None,
        };
        let default = Theme::Default.palette();
        let line = build_bar_line(&w, 3, false, false, 80, &default);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("["), "text was: {text}");
        assert!(text.contains("]"));
    }

    #[test]
    fn hint_line_includes_theme_hint() {
        let p = Theme::Default.palette();
        let line = build_hint_line(false, &p);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("Theme"), "text was: {text}");
        assert!(text.contains("Refresh"));
        assert!(text.contains("Quit"));
    }

    #[test]
    fn draw_renders_all_themes() {
        use crate::app::{AppState, SourceSlot, Tab};
        use crate::model::ProviderState;
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        use tokio::sync::mpsc;

        for theme in [Theme::Default, Theme::Crush, Theme::Btop] {
            let (tx, _rx) = mpsc::channel(4);
            let tab = Tab {
                provider: Provider::Claude,
                sources: vec![SourceSlot {
                    id: "oauth".into(),
                    state: SourceState::Quota(ProviderState {
                        provider: Provider::Claude,
                        label: "Claude (Pro)".into(),
                        windows: vec![
                            LimitWindow {
                                kind: WindowKind::FiveHours,
                                scope: None,
                                used: 580,
                                limit: 1000,
                                reset_at: None,
                            },
                            LimitWindow {
                                kind: WindowKind::SevenDays,
                                scope: None,
                                used: 950,
                                limit: 1000,
                                reset_at: None,
                            },
                        ],
                        last_updated: None,
                        last_error: None,
                    }),
                    poll_tx: tx,
                    last_poll_at: None,
                    next_poll_at: None,
                }],
                active: 0,
            };
            let mut app = AppState::new(vec![tab]);
            app.theme = theme;

            let backend = TestBackend::new(100, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|f| draw(f, &app)).unwrap();

            let buf = terminal.backend().buffer();
            let mut lines = Vec::new();
            for y in 0..10usize {
                let line: String = (0..100usize)
                    .map(|x| buf[(x as u16, y as u16)].symbol().to_string())
                    .collect();
                lines.push(line);
            }
            let frame = lines.join("\n");
            assert!(frame.contains("AIBar"), "{theme:?}: missing AIBar");
            assert!(
                frame.contains("Claude (Pro)"),
                "{theme:?}: missing tab label"
            );
            assert!(frame.contains("58%"), "{theme:?}: missing bar data");
            assert!(frame.contains("95%"), "{theme:?}: missing second bar");
            assert!(frame.contains("Theme"), "{theme:?}: missing hint");
            match theme {
                Theme::Crush => {
                    assert!(
                        frame.contains('\u{256d}'),
                        "crush should use rounded corners"
                    );
                    assert_eq!(buf[(0u16, 0u16)].bg, crate::theme::test_bg());
                    assert_eq!(
                        buf[(0u16, 9u16)].bg,
                        crate::theme::test_bg(),
                        "bottom border row also painted"
                    );
                }
                _ => {
                    assert!(frame.contains('\u{250c}'));
                    assert_eq!(
                        buf[(50u16, 5u16)].bg,
                        ratatui::style::Color::Reset,
                        "other themes keep terminal bg"
                    );
                }
            }
        }
    }
}
