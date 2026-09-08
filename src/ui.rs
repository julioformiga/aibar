use crate::app::{AppState, ClickTarget, FooterAction};
use crate::model::{
    CreditsState, HyperPlan, LimitScope, LimitWindow, Provider, SourceState, WindowKind,
};
use crate::theme::Palette;
use chrono::{DateTime, Utc};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use ratatui::Frame;
use std::ops::Range;
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
            .title_bottom(build_hint_line(has_multi, app.watch_mode, &palette).right_aligned());
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
    breadcrumb_parts(app, p).0
}

/// Builds the top-breadcrumb line alongside the column range (in line-local
/// coordinates) each tab label occupies, so drawing and mouse hit-testing
/// can never drift apart. A tab range covers " <label> " including padding.
fn breadcrumb_parts(app: &AppState, p: &Palette) -> (Line<'static>, Vec<(usize, Range<usize>)>) {
    let mut spans = vec![Span::styled(" AIBar ", p.title)];
    let mut targets = Vec::new();
    let mut x = spans[0].width();
    for (i, tab) in app.tabs.iter().enumerate() {
        spans.push(tab_separator(p));
        x += 1;
        let start = x;
        spans.push(Span::raw(" "));
        x += 1;
        let label = tab
            .active_state()
            .map(|s| s.label().to_string())
            .unwrap_or_else(|| tab.provider.label().to_string());
        if i == app.active_tab {
            let span = Span::styled(label, p.tab_active);
            x += span.width();
            spans.push(span);
        } else {
            let span = Span::styled(label, p.tab_inactive);
            x += span.width();
            spans.push(span);
        }
        spans.push(Span::raw(" "));
        x += 1;
        targets.push((i, start..x));
    }
    spans.push(Span::raw(" "));
    (Line::from(spans), targets)
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

fn build_hint_line(has_multi: bool, watch_mode: bool, p: &Palette) -> Line<'static> {
    hint_parts(has_multi, watch_mode, p).0
}

/// Builds the right-aligned footer hint line alongside the column range (in
/// line-local coordinates) each clickable action occupies, covering the key
/// plus its full label text (e.g. "W Watch (on)").
fn hint_parts(
    has_multi: bool,
    watch_mode: bool,
    p: &Palette,
) -> (Line<'static>, Vec<(FooterAction, Range<usize>)>) {
    let bold = p.hint_key;
    let sep = "  ";

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut targets = Vec::new();
    let mut x = 0usize;

    if has_multi {
        push_hint_group(
            &mut spans,
            &mut x,
            &mut targets,
            FooterAction::CycleSource,
            vec![Span::styled("E", bold), hint_text(p, "nter Source")],
        );
        spans.push(Span::raw(sep));
        x += sep.len();
    }
    let watch_state = if watch_mode {
        Span::styled("(on)", bold)
    } else {
        hint_text(p, "(off)")
    };
    push_hint_group(
        &mut spans,
        &mut x,
        &mut targets,
        FooterAction::CycleTheme,
        vec![
            Span::styled("\u{2191}\u{2193}", bold),
            hint_text(p, " Theme"),
        ],
    );
    spans.push(Span::raw(sep));
    x += sep.len();
    push_hint_group(
        &mut spans,
        &mut x,
        &mut targets,
        FooterAction::ToggleWatch,
        vec![Span::styled("W", bold), hint_text(p, "atch "), watch_state],
    );
    spans.push(Span::raw(sep));
    x += sep.len();
    push_hint_group(
        &mut spans,
        &mut x,
        &mut targets,
        FooterAction::Refresh,
        vec![Span::styled("R", bold), hint_text(p, "efresh")],
    );
    spans.push(Span::raw(sep));
    x += sep.len();
    push_hint_group(
        &mut spans,
        &mut x,
        &mut targets,
        FooterAction::Quit,
        vec![Span::styled("Q", bold), hint_text(p, "uit")],
    );

    (Line::from(spans), targets)
}

fn push_hint_group(
    spans: &mut Vec<Span<'static>>,
    x: &mut usize,
    targets: &mut Vec<(FooterAction, Range<usize>)>,
    action: FooterAction,
    parts: Vec<Span<'static>>,
) {
    let start = *x;
    for part in parts {
        *x += part.width();
        spans.push(part);
    }
    targets.push((action, start..*x));
}

/// Maps a click at `col`/`row` (terminal coordinates) to the tab it lands on
/// (top border row) or the footer action it lands on (bottom border row).
/// Returns `None` for anything outside those two rows or outside every range.
pub fn hit_test(app: &AppState, area: Rect, col: u16, row: u16) -> Option<ClickTarget> {
    if app.is_empty() || area.height == 0 || area.width == 0 {
        return None;
    }
    let palette = app.theme.palette();
    let col = col as usize;

    if row == 0 {
        if col == 0 {
            return None;
        }
        let (_, tabs) = breadcrumb_parts(app, &palette);
        let col = col - 1;
        return tabs
            .iter()
            .find(|(_, range)| range.contains(&col))
            .map(|(idx, _)| ClickTarget::Tab(*idx));
    }

    if row == area.height - 1 {
        let has_multi = app
            .tabs
            .get(app.active_tab)
            .map(|t| t.sources.len() > 1)
            .unwrap_or(false);
        let (line, actions) = hint_parts(has_multi, app.watch_mode, &palette);
        let inner = (area.width as usize).saturating_sub(2);
        let start = 1 + inner.saturating_sub(line.width());
        if col < start {
            return None;
        }
        let col = col - start;
        return actions
            .iter()
            .find(|(_, range)| range.contains(&col))
            .map(|(action, _)| ClickTarget::Footer(*action));
    }

    None
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
    if ps.provider == Provider::OpenAI && !has_data {
        let message = if ps.last_updated.is_some() || ps.last_error.is_some() {
            "  Codex quota unavailable"
        } else {
            "  Loading Codex quota\u{2026}"
        };
        return vec![Line::from(Span::styled(
            message,
            Style::default().fg(p.loading),
        ))];
    }

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
            lines.push(build_bar_line(
                w,
                label_w,
                show_warn,
                cached,
                ps.provider != Provider::OpenAI,
                width,
                p,
            ));
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
    let plan = cs.resolved_plan();
    let allowance = cs.allowance().max(balance);
    let spent = (allowance - balance).max(0.0);
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

    // Só o plano mensal tem cadência conhecida (diária); o gratuito não
    // reserva a coluna de countdown.
    let countdown = match plan {
        HyperPlan::Monthly => Some(format!(
            "{:>width$} ",
            reset_countdown(cs.reset_at),
            width = TIME_W - 3
        )),
        HyperPlan::Free => None,
    };

    let prefix_len = 2 + label_w + 1 + countdown.as_deref().map_or(0, |c| c.len());
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
    if let Some(c) = countdown {
        spans.push(metrics_span(c, p));
    }
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
    spans.push(metrics_span(
        format!(" {:<width$}", suffix, width = SUFFIX_W),
        p,
    ));

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
    show_counts: bool,
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
    } else if show_counts {
        format!("{}/{}", fmt_count(window.used), fmt_count(window.limit))
    } else {
        String::new()
    };

    let time_part = format!("{:>width$}", countdown, width = TIME_W - 3);
    let kind_part = format!("/{}", kind_str);
    let suffix_part = if show_counts {
        format!(" {:<width$}", suffix, width = SUFFIX_W)
    } else if cached {
        format!(" {}", suffix)
    } else {
        String::new()
    };
    let prefix_len = 2 + label_w + 1 + time_part.len() + kind_part.len();
    let suffix_len = pct_str.len() + suffix_part.len();
    let bar_width = if show_counts {
        // Preserve the existing spacing and minimum bar for other providers.
        width.saturating_sub(prefix_len + suffix_len + 5).max(10)
    } else {
        let brackets = Span::raw(p.bar_open).width() + Span::raw(p.bar_close).width();
        width.saturating_sub(prefix_len + suffix_len + brackets)
    };

    let filled = ((bar_width as f32) * (pct / 100.0)).round() as usize;
    let filled = filled.min(bar_width);
    let empty = bar_width - filled;

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
    spans.push(metrics_span(time_part, p));
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
    spans.push(metrics_span(suffix_part, p));

    Line::from(spans)
}

fn metrics_span(s: String, p: &Palette) -> Span<'static> {
    match p.metrics {
        Some(c) => Span::styled(s, Style::default().fg(c)),
        None => Span::raw(s),
    }
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
    spans.push(metrics_span(
        format!(" {:>width$}", "--", width = TIME_W - 3),
        p,
    ));
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
        Line::from("  pass Z_AI_API_KEY, HYPER_API_KEY, Antigravity (agy),"),
        Line::from("  codex (OpenAI Codex CLI, signed in with ChatGPT)"),
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

fn window_kind_str(kind: WindowKind) -> String {
    match kind {
        WindowKind::FiveHours => "5h".into(),
        WindowKind::SevenDays => "7d".into(),
        WindowKind::Minutes(m) if m > 0 && m % 1440 == 0 => format!("{}d", m / 1440),
        WindowKind::Minutes(m) if m > 0 && m % 60 == 0 => format!("{}h", m / 60),
        WindowKind::Minutes(m) => format!("{}m", m),
        WindowKind::Unknown => "?".into(),
    }
}

fn placeholder_labels(provider: Provider) -> Vec<String> {
    match provider {
        Provider::Claude | Provider::Zai => vec!["".to_string(), "".to_string()],
        Provider::Hyper | Provider::OpenAI => vec![],
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
        Provider::Hyper | Provider::OpenAI => vec![],
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
        credits_state_with_plan(balance, None, None, last_error)
    }

    fn credits_state_with_plan(
        balance: Option<f64>,
        plan: Option<HyperPlan>,
        reset_at: Option<DateTime<Utc>>,
        last_error: Option<String>,
    ) -> CreditsState {
        CreditsState {
            label: "Hyper".into(),
            balance,
            plan,
            reset_at,
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
        let _guard = crate::config::env_var_test_lock().lock().unwrap();
        std::env::remove_var("AIBAR_HYPER_PLAN");
        let p = Theme::Default.palette();
        let line = build_credits_line(&credits_state(Some(30.0), None), 80, &p);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains(" 70%"), "text was: {text}");
        assert!(text.contains("bal 30"), "text was: {text}");
    }

    #[test]
    fn credits_line_full_monthly_balance_is_zero_spent() {
        let _guard = crate::config::env_var_test_lock().lock().unwrap();
        std::env::remove_var("AIBAR_HYPER_PLAN");
        let p = Theme::Default.palette();
        let line = build_credits_line(&credits_state(Some(250.0), None), 80, &p);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("  0%"), "text was: {text}");
        assert!(text.contains("bal 250"), "text was: {text}");
    }

    #[test]
    fn credits_line_monthly_uses_daily_allowance() {
        let _guard = crate::config::env_var_test_lock().lock().unwrap();
        std::env::remove_var("AIBAR_HYPER_PLAN");
        let p = Theme::Default.palette();
        let cs = credits_state_with_plan(Some(109.0), Some(HyperPlan::Monthly), None, None);
        let line = build_credits_line(&cs, 80, &p);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains(" 56%"), "text was: {text}");
        assert!(text.contains("bal 109"), "text was: {text}");
        assert!(text.contains("--"), "no anchor yet, text was: {text}");
    }

    #[test]
    fn credits_line_monthly_shows_countdown_with_anchor() {
        let _guard = crate::config::env_var_test_lock().lock().unwrap();
        std::env::remove_var("AIBAR_HYPER_PLAN");
        let p = Theme::Default.palette();
        let reset_at = Some(Utc::now() + ChronoDuration::seconds(2 * 3_600 + 300));
        let cs = credits_state_with_plan(Some(109.0), Some(HyperPlan::Monthly), reset_at, None);
        let line = build_credits_line(&cs, 80, &p);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("2h"), "text was: {text}");
    }

    #[test]
    fn credits_line_free_plan_has_no_countdown_column() {
        let _guard = crate::config::env_var_test_lock().lock().unwrap();
        std::env::remove_var("AIBAR_HYPER_PLAN");
        let p = Theme::Default.palette();
        let cs = credits_state_with_plan(Some(30.0), Some(HyperPlan::Free), None, None);
        let line = build_credits_line(&cs, 80, &p);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(!text.contains("--"), "text was: {text}");
        assert!(text.contains(" 70%"), "text was: {text}");
    }

    #[test]
    fn credits_line_env_override_forces_plan() {
        let _guard = crate::config::env_var_test_lock().lock().unwrap();
        std::env::set_var("AIBAR_HYPER_PLAN", "monthly");
        let p = Theme::Default.palette();
        // Sem override seria Free: (100-80)/100 = 20%.
        let line = build_credits_line(&credits_state(Some(80.0), None), 80, &p);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains(" 68%"), "text was: {text}");
        std::env::remove_var("AIBAR_HYPER_PLAN");
    }

    #[test]
    fn credits_line_formats_fractional_balance() {
        let _guard = crate::config::env_var_test_lock().lock().unwrap();
        std::env::remove_var("AIBAR_HYPER_PLAN");
        let p = Theme::Default.palette();
        let line = build_credits_line(&credits_state(Some(42.5), None), 80, &p);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains(" 58%"), "text was: {text}");
        assert!(text.contains("bal 42.5"), "text was: {text}");
    }

    #[test]
    fn credits_line_cached_shows_warn_and_cached_suffix() {
        let _guard = crate::config::env_var_test_lock().lock().unwrap();
        std::env::remove_var("AIBAR_HYPER_PLAN");
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
        let line = build_bar_line(&w, 7, false, false, true, 80, &btop);
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
        let line = build_bar_line(&w, 3, false, false, true, 80, &default);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("["), "text was: {text}");
        assert!(text.contains("]"));
    }

    fn openai_state(windows: Vec<LimitWindow>, updated: bool, error: Option<&str>) -> SourceState {
        SourceState::Quota(crate::model::ProviderState {
            provider: Provider::OpenAI,
            label: "OpenAI (Codex Plus)".into(),
            windows,
            last_updated: updated.then(Utc::now),
            last_error: error.map(|e| e.to_string()),
        })
    }

    fn quota_text(state: &SourceState, width: usize) -> String {
        let SourceState::Quota(ps) = state else {
            panic!("expected Quota state");
        };
        draw_quota_lines(ps, width, &Theme::Default.palette())
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn codex_bars_show_percent_and_reset_without_fake_counts() {
        let state = openai_state(
            vec![
                // The extra second keeps the countdown from rounding down
                // while the test runs.
                LimitWindow::from_fraction(
                    WindowKind::FiveHours,
                    None,
                    0.95,
                    Some(Utc::now() + ChronoDuration::minutes(150) + ChronoDuration::seconds(1)),
                ),
                LimitWindow::from_fraction(
                    WindowKind::SevenDays,
                    None,
                    0.15,
                    Some(Utc::now() + ChronoDuration::hours(80) + ChronoDuration::seconds(1)),
                ),
            ],
            true,
            None,
        );

        let text = quota_text(&state, 80);
        assert!(text.contains(" 95%"), "text was: {text}");
        assert!(text.contains(" 15%"), "text was: {text}");
        assert!(text.contains("/5h"), "text was: {text}");
        assert!(text.contains("/7d"), "text was: {text}");
        assert!(text.contains("2h30m"), "text was: {text}");
        assert!(text.contains("3d08h"), "text was: {text}");
        // The notional 1000 limit is an internal scaling detail, not a real
        // message count, so it must never reach the Codex tab.
        assert!(!text.contains("/1k"), "text was: {text}");
    }

    #[test]
    fn codex_renders_dynamic_window_durations() {
        let state = openai_state(
            vec![
                LimitWindow::from_fraction(WindowKind::Minutes(90), None, 0.2, None),
                LimitWindow::from_fraction(WindowKind::Minutes(720), None, 0.3, None),
                LimitWindow::from_fraction(WindowKind::Unknown, None, 0.4, None),
            ],
            true,
            None,
        );

        let text = quota_text(&state, 80);
        assert!(text.contains("/90m"), "text was: {text}");
        assert!(text.contains("/12h"), "text was: {text}");
        assert!(text.contains("/?"), "text was: {text}");
    }

    #[test]
    fn codex_empty_state_distinguishes_loading_from_unavailable() {
        let loading = quota_text(&openai_state(vec![], false, None), 80);
        assert!(
            loading.contains("Loading Codex quota"),
            "text was: {loading}"
        );

        let empty_response = quota_text(&openai_state(vec![], true, None), 80);
        assert!(
            empty_response.contains("Codex quota unavailable"),
            "text was: {empty_response}"
        );

        let failed = quota_text(
            &openai_state(
                vec![],
                false,
                Some("codex login required: run `codex login`"),
            ),
            80,
        );
        assert!(
            failed.contains("Codex quota unavailable"),
            "text was: {failed}"
        );

        // No invented 5h/7d placeholder bars before Codex answers.
        for text in [loading, empty_response, failed] {
            assert!(!text.contains("/5h"), "text was: {text}");
            assert!(!text.contains("/7d"), "text was: {text}");
        }
    }

    #[test]
    fn codex_bars_survive_narrow_terminals() {
        let state = openai_state(
            vec![LimitWindow::from_fraction(
                WindowKind::FiveHours,
                None,
                0.95,
                None,
            )],
            true,
            None,
        );

        for width in [0usize, 1, 8, 20, 40] {
            let text = quota_text(&state, width);
            assert!(text.contains("95%"), "width {width} produced: {text}");
        }
    }

    #[test]
    fn codex_cached_state_marks_stale_data() {
        let state = openai_state(
            vec![LimitWindow::from_fraction(
                WindowKind::FiveHours,
                None,
                0.5,
                None,
            )],
            true,
            Some("request timed out"),
        );

        let text = quota_text(&state, 80);
        assert!(text.contains("(cached)"), "text was: {text}");
        assert!(text.contains(WARN), "text was: {text}");
    }

    #[test]
    fn hint_line_includes_theme_hint() {
        let p = Theme::Default.palette();
        let line = build_hint_line(false, false, &p);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("Theme"), "text was: {text}");
        assert!(text.contains("Refresh"));
        assert!(text.contains("Quit"));
    }

    #[test]
    fn hint_line_shows_watch_state() {
        let p = Theme::Default.palette();

        let off = build_hint_line(false, false, &p);
        let off_text: String = off.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(off_text.contains("Watch (off)"), "text was: {off_text}");

        let on = build_hint_line(false, true, &p);
        let on_text: String = on.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(on_text.contains("Watch (on)"), "text was: {on_text}");
    }

    #[test]
    fn draw_renders_all_themes() {
        use crate::app::{AppState, SourceSlot, Tab};
        use crate::model::ProviderState;
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        use tokio::sync::mpsc;

        for theme in [Theme::Default, Theme::Crush, Theme::Btop, Theme::Opencode] {
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
                Theme::Opencode => {
                    assert!(
                        frame.contains('\u{256d}'),
                        "opencode should use rounded corners"
                    );
                    assert_eq!(buf[(0u16, 0u16)].bg, crate::theme::test_opencode_bg());
                    assert_eq!(
                        buf[(0u16, 9u16)].bg,
                        crate::theme::test_opencode_bg(),
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

    #[test]
    fn hit_test_matches_rendered_layout() {
        use crate::app::{AppState, ClickTarget, FooterAction, SourceSlot, Tab};
        use crate::model::ProviderState;
        use ratatui::backend::TestBackend;
        use ratatui::layout::Rect;
        use ratatui::Terminal;
        use tokio::sync::mpsc;

        let make_slot = |provider: Provider, id: &str| -> SourceSlot {
            let (tx, _rx) = mpsc::channel(4);
            SourceSlot {
                id: id.into(),
                state: SourceState::Quota(ProviderState {
                    provider,
                    label: provider.label().to_string(),
                    windows: vec![],
                    last_updated: None,
                    last_error: None,
                }),
                poll_tx: tx,
                last_poll_at: None,
                next_poll_at: None,
            }
        };
        let claude = Tab {
            provider: Provider::Claude,
            sources: vec![
                make_slot(Provider::Claude, "oauth"),
                make_slot(Provider::Claude, "api"),
            ],
            active: 0,
        };
        let zai = Tab {
            provider: Provider::Zai,
            sources: vec![make_slot(Provider::Zai, "default")],
            active: 0,
        };
        let app = AppState::new(vec![claude, zai]);

        let backend = TestBackend::new(100, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();

        let area = Rect::new(0, 0, 100, 10);
        let row = |y: u16| -> String {
            let buf = terminal.backend().buffer();
            (0..100usize)
                .map(|x| buf[(x as u16, y)].symbol().to_string())
                .collect()
        };
        let top = row(0);
        let bottom = row(9);
        let click = |col: usize, r: u16| hit_test(&app, area, col as u16, r);
        // Every rendered cell is one column wide, but box glyphs are
        // multi-byte, so search by char index (== screen column), not bytes.
        let find_col = |hay: &str, needle: &str| -> usize {
            let hay: Vec<char> = hay.chars().collect();
            let needle: Vec<char> = needle.chars().collect();
            (0..=hay.len().saturating_sub(needle.len()))
                .find(|&i| hay[i..i + needle.len()] == needle[..])
                .unwrap()
        };

        let col = find_col(&top, "Claude");
        assert_eq!(click(col, 0), Some(ClickTarget::Tab(0)));
        let col = find_col(&top, "Z.ai");
        assert_eq!(click(col, 0), Some(ClickTarget::Tab(1)));

        // Non-tab regions of the title row are not clickable.
        assert_eq!(click(find_col(&top, "AIBar"), 0), None);
        assert_eq!(click(find_col(&top, "\u{2502}"), 0), None);

        let col = find_col(&bottom, "Source");
        assert_eq!(
            click(col, 9),
            Some(ClickTarget::Footer(FooterAction::CycleSource))
        );
        let col = find_col(&bottom, "Theme");
        assert_eq!(
            click(col, 9),
            Some(ClickTarget::Footer(FooterAction::CycleTheme))
        );
        let col = find_col(&bottom, "Watch");
        assert_eq!(
            click(col, 9),
            Some(ClickTarget::Footer(FooterAction::ToggleWatch))
        );
        let col = find_col(&bottom, "Refresh");
        assert_eq!(
            click(col, 9),
            Some(ClickTarget::Footer(FooterAction::Refresh))
        );
        let col = find_col(&bottom, "Quit");
        assert_eq!(click(col, 9), Some(ClickTarget::Footer(FooterAction::Quit)));

        // Status area on the bottom row and content rows are inert.
        assert_eq!(click(0, 9), None);
        assert_eq!(click(50, 5), None);
    }
}
