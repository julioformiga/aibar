use crate::app::AppState;
use crate::config::color_for_percentage;
use crate::model::{LimitScope, LimitWindow, Provider, SourceState, WindowKind};
use chrono::{DateTime, Utc};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
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
    let area = f.area();
    let title = breadcrumb_title(app);
    let mut block = Block::default().borders(Borders::ALL).title(title);

    if !app.is_empty() {
        if let Some(t) = build_timer_line(app) {
            block = block.title(t.right_aligned());
        }
        let has_multi = app
            .tabs
            .get(app.active_tab)
            .map(|t| t.sources.len() > 1)
            .unwrap_or(false);
        block = block
            .title_bottom(build_status_line(app))
            .title_bottom(build_hint_line(has_multi).right_aligned());
    }

    let inner = block.inner(area);
    block.render(area, f.buffer_mut());

    if app.is_empty() {
        draw_welcome(f, inner);
        return;
    }

    draw_content(f, app, inner);
}

fn breadcrumb_title(app: &AppState) -> Line<'static> {
    let mut spans = vec![
        Span::styled(
            " AIBar ",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Cyan),
        ),
        Span::raw("\u{2502}"),
    ];
    for (i, tab) in app.tabs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("\u{2502}"));
        }
        spans.push(Span::raw(" "));
        let label = tab
            .active_state()
            .map(|s| s.label().to_string())
            .unwrap_or_else(|| tab.provider.label().to_string());
        if i == app.active_tab {
            spans.push(Span::styled(
                label,
                Style::default()
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                    .fg(Color::Yellow),
            ));
        } else {
            spans.push(Span::raw(label));
        }
        spans.push(Span::raw(" "));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn build_status_line(app: &AppState) -> Line<'static> {
    let active = app.active_state();
    if let Some(err) = active.and_then(|s| s.last_error()) {
        Line::from(Span::styled(
            format!(" {}  {} ", WARN, err),
            Style::default().fg(Color::Red),
        ))
    } else if let Some(msg) = &app.status_message {
        Line::from(Span::styled(
            format!(" {} ", msg),
            Style::default().fg(Color::DarkGray),
        ))
    } else {
        Line::from("")
    }
}

fn build_hint_line(has_multi: bool) -> Line<'static> {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let sep = "  ";

    let mut spans = Vec::new();
    if has_multi {
        spans.extend_from_slice(&[
            Span::styled("E", bold),
            Span::raw("nter Source"),
            Span::raw(sep),
        ]);
    }
    spans.extend_from_slice(&[
        Span::styled("R", bold),
        Span::raw("efresh"),
        Span::raw(sep),
        Span::styled("Q", bold),
        Span::raw("uit"),
    ]);

    Line::from(spans)
}

fn build_timer_line(app: &AppState) -> Option<Line<'static>> {
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

    let spans = vec![
        Span::raw(repeat_char(TIMER_FILLED, filled)),
        Span::raw(repeat_char(TIMER_EMPTY, empty)),
    ];

    Some(Line::from(spans))
}

fn draw_content(f: &mut Frame, app: &AppState, area: Rect) {
    let Some(state) = app.active_state() else {
        return;
    };

    let width = area.width as usize;
    let lines = match state {
        SourceState::Quota(ps) => draw_quota_lines(ps, width),
        SourceState::Ceiling(cr) => draw_ceiling_lines(cr, width),
    };

    Paragraph::new(lines).render(area, f.buffer_mut());
}

fn draw_quota_lines(ps: &crate::model::ProviderState, width: usize) -> Vec<Line<'static>> {
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
            lines.push(build_bar_line(w, label_w, show_warn, cached, width));
        }
    } else {
        let kinds = placeholder_kinds(ps.provider);
        for (label, kind) in labels.iter().zip(kinds.iter()) {
            lines.push(build_loading_line(label, *kind, label_w, width));
        }
    }
    lines
}

fn draw_ceiling_lines(cr: &crate::model::CeilingReport, _width: usize) -> Vec<Line<'static>> {
    if cr.ceilings.is_empty() {
        let mut spans = Vec::new();
        if cr.last_error.is_some() {
            spans.push(Span::styled(
                format!(" {}  ", WARN),
                Style::default().fg(Color::Yellow),
            ));
        }
        spans.push(Span::styled(
            "Loading rate limits\u{2026}",
            Style::default().fg(Color::DarkGray),
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
            Style::default().fg(Color::Cyan),
        ));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{:>6} in", fmt_count(c.in_tpm)),
            Style::default().fg(Color::Green),
        ));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{:>6} out", fmt_count(c.out_tpm)),
            Style::default().fg(Color::Yellow),
        ));
        lines.push(Line::from(spans));
    }
    lines
}

fn build_bar_line(
    window: &LimitWindow,
    label_w: usize,
    show_warn: bool,
    cached: bool,
    width: usize,
) -> Line<'static> {
    let pct = window.percentage();
    let color = color_for_percentage(pct);
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
            Style::default().fg(Color::Yellow),
        ));
    } else {
        spans.push(Span::raw("  "));
    }

    spans.push(Span::raw(format!("{:<width$}", label, width = label_w)));
    spans.push(Span::raw(" "));
    spans.push(Span::raw(time_part));
    spans.push(Span::styled(
        kind_part,
        Style::default().fg(Color::DarkGray),
    ));
    spans.push(Span::raw(" ["));
    spans.push(Span::styled(
        repeat_char('\u{2588}', filled),
        Style::default().fg(color),
    ));
    spans.push(Span::styled(
        repeat_char('\u{2591}', empty),
        Style::default().fg(Color::DarkGray),
    ));
    spans.push(Span::raw("] "));
    spans.push(Span::styled(pct_str, Style::default().fg(color)));
    spans.push(Span::raw(format!(" {:<width$}", suffix, width = SUFFIX_W)));

    Line::from(spans)
}

fn build_loading_line(
    label: &str,
    kind: WindowKind,
    label_w: usize,
    width: usize,
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
        Style::default().fg(Color::DarkGray),
    ));
    spans.push(Span::raw(" ["));
    spans.push(Span::styled(
        repeat_char('\u{2591}', bar_width),
        Style::default().fg(Color::DarkGray),
    ));
    spans.push(Span::raw("] "));
    spans.push(Span::styled(
        "\u{2014}",
        Style::default().fg(Color::DarkGray),
    ));

    Line::from(spans)
}

fn draw_welcome(f: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "No provider detected.",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Set at least one of these environment variables:"),
        Line::from(""),
        Line::from("  export ANTHROPIC_API_KEY=\"...\"   # Claude (API)"),
        Line::from("  export ZAI_API_KEY=\"...\"         # Z.ai"),
        Line::from("  export GEMINI_API_KEY=\"...\"      # Gemini"),
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

fn repeat_char(ch: char, n: usize) -> String {
    std::iter::repeat_n(ch, n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
