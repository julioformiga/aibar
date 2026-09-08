use crate::config::{color_for_percentage, COLOR_HIGH_THRESHOLD, COLOR_LOW_THRESHOLD};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::BorderType;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    Default,
    Crush,
    Btop,
    Opencode,
}

impl Theme {
    pub fn label(self) -> &'static str {
        match self {
            Theme::Default => "default",
            Theme::Crush => "crush",
            Theme::Btop => "btop",
            Theme::Opencode => "opencode",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Theme::Default => Theme::Crush,
            Theme::Crush => Theme::Btop,
            Theme::Btop => Theme::Opencode,
            Theme::Opencode => Theme::Default,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Theme::Default => Theme::Opencode,
            Theme::Crush => Theme::Default,
            Theme::Btop => Theme::Crush,
            Theme::Opencode => Theme::Btop,
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            Theme::Default => default_palette(),
            Theme::Crush => crush_palette(),
            Theme::Btop => btop_palette(),
            Theme::Opencode => opencode_palette(),
        }
    }

    pub fn for_provider(label: &str) -> Self {
        match label {
            "Claude" => Theme::Default,
            "Z.ai" => Theme::Opencode,
            "Hyper" => Theme::Crush,
            "Gemini" => Theme::Btop,
            "OpenAI" => Theme::Opencode,
            _ => Theme::Default,
        }
    }
}

pub struct Palette {
    pub bg: Option<Color>,
    pub border: BorderType,
    pub border_color: Option<Color>,
    pub title: Style,
    pub tab_active: Style,
    pub tab_inactive: Style,
    pub tab_separator: Option<Color>,
    pub bar_open: &'static str,
    pub bar_close: &'static str,
    pub filled_char: char,
    pub empty_char: char,
    pub empty_style: Style,
    pub bar_fill: fn(pct: f32, pos: f32) -> Color,
    pub pct_color: fn(pct: f32) -> Color,
    pub kind: Color,
    pub status: Color,
    /// Explicit fg for time/token count indicators (countdown, `used/limit`,
    /// balance). `None` keeps the terminal's default foreground.
    pub metrics: Option<Color>,
    pub error: Color,
    pub warn: Color,
    pub hint_key: Style,
    pub hint_text: Option<Color>,
    pub timer_filled: Option<fn(pos: f32) -> Color>,
    pub timer_empty: Option<Color>,
    pub loading: Color,
    pub ceiling_rpm: Color,
    pub ceiling_in: Color,
    pub ceiling_out: Color,
    pub welcome_title: Style,
}

fn rgb(hex: u32) -> Color {
    Color::Rgb(
        ((hex >> 16) & 0xFF) as u8,
        ((hex >> 8) & 0xFF) as u8,
        (hex & 0xFF) as u8,
    )
}

fn lerp_channel(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => Color::Rgb(
            lerp_channel(ar, br, t),
            lerp_channel(ag, bg, t),
            lerp_channel(ab, bb, t),
        ),
        _ => a,
    }
}

fn solid_by_pct(pct: f32, _pos: f32) -> Color {
    color_for_percentage(pct)
}

fn crush_severity(pct: f32) -> Color {
    if pct <= COLOR_LOW_THRESHOLD {
        rgb(0x00FFB2)
    } else if pct <= COLOR_HIGH_THRESHOLD {
        rgb(0xF5EF34)
    } else {
        rgb(0xEB4268)
    }
}

fn crush_gradient(_pct: f32, pos: f32) -> Color {
    lerp_color(rgb(0x8B75FF), rgb(0xFF60FF), pos.clamp(0.0, 1.0))
}

const BTOP_CPU: [u32; 3] = [0x77CA9B, 0xCBC06C, 0xDC4C4C];

fn btop_gradient_at(t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.5 {
        lerp_color(rgb(BTOP_CPU[0]), rgb(BTOP_CPU[1]), t * 2.0)
    } else {
        lerp_color(rgb(BTOP_CPU[1]), rgb(BTOP_CPU[2]), (t - 0.5) * 2.0)
    }
}

fn btop_gradient(_pct: f32, pos: f32) -> Color {
    btop_gradient_at(pos)
}

fn timer_crush(pos: f32) -> Color {
    lerp_color(rgb(0x8B75FF), rgb(0xFF60FF), pos.clamp(0.0, 1.0))
}

fn timer_btop(_pos: f32) -> Color {
    rgb(0xCBC06C)
}

// OpenCode default theme, dark variant (packages/tui/src/theme/assets/opencode.json):
// mostly grays — bg #0A0A0A, border #484848, borderSubtle #3C3C3C, text #EEEEEE,
// textMuted #808080 — with secondary blue #5C9CF5 and accent purple #9D7CD8
// reserved for the timer gradient; bars/percentages reuse the same blue.
fn opencode_blue(_pct: f32) -> Color {
    rgb(0x5C9CF5)
}

fn opencode_bar_blue(_pct: f32, _pos: f32) -> Color {
    opencode_blue(_pct)
}

fn timer_opencode(pos: f32) -> Color {
    lerp_color(rgb(0x5C9CF5), rgb(0x9D7CD8), pos.clamp(0.0, 1.0))
}

fn btop_pct(pct: f32) -> Color {
    btop_gradient_at(pct / 100.0)
}

#[cfg(test)]
pub(crate) fn test_bg() -> Color {
    rgb(0x1F1C23)
}

#[cfg(test)]
pub(crate) fn test_opencode_bg() -> Color {
    rgb(0x0A0A0A)
}

fn default_palette() -> Palette {
    Palette {
        bg: None,
        border: BorderType::Plain,
        border_color: None,
        title: Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Cyan),
        tab_active: Style::default()
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            .fg(Color::Yellow),
        tab_inactive: Style::default(),
        tab_separator: None,
        bar_open: " [",
        bar_close: "] ",
        filled_char: '\u{2588}',
        empty_char: '\u{2591}',
        empty_style: Style::default().fg(Color::DarkGray),
        bar_fill: solid_by_pct,
        pct_color: color_for_percentage,
        kind: Color::DarkGray,
        status: Color::DarkGray,
        metrics: None,
        error: Color::Red,
        warn: Color::Yellow,
        hint_key: Style::default().add_modifier(Modifier::BOLD),
        hint_text: None,
        timer_filled: None,
        timer_empty: None,
        loading: Color::DarkGray,
        ceiling_rpm: Color::Cyan,
        ceiling_in: Color::Green,
        ceiling_out: Color::Yellow,
        welcome_title: Style::default().add_modifier(Modifier::BOLD),
    }
}

fn crush_palette() -> Palette {
    Palette {
        bg: Some(rgb(0x1F1C23)),
        border: BorderType::Rounded,
        border_color: Some(rgb(0x8B75FF)),
        title: Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(rgb(0xF7F6FB)),
        tab_active: Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(rgb(0xFF60FF)),
        tab_inactive: Style::default().fg(rgb(0x858392)),
        tab_separator: Some(rgb(0x4D4C57)),
        bar_open: " (",
        bar_close: ") ",
        filled_char: '\u{2501}',
        empty_char: '\u{2504}',
        empty_style: Style::default().fg(rgb(0x858392)),
        bar_fill: crush_gradient,
        pct_color: crush_severity,
        kind: rgb(0x858392),
        status: rgb(0x858392),
        metrics: None,
        error: rgb(0xEB4268),
        warn: rgb(0xF5EF34),
        hint_key: Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(rgb(0xFF84FF)),
        hint_text: Some(rgb(0xBFBCC8)),
        timer_filled: Some(timer_crush),
        timer_empty: Some(rgb(0x858392)),
        loading: rgb(0x858392),
        ceiling_rpm: rgb(0x00A4FF),
        ceiling_in: rgb(0x00FFB2),
        ceiling_out: rgb(0xF5EF34),
        welcome_title: Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(rgb(0xECEBF0)),
    }
}

fn btop_palette() -> Palette {
    let track = rgb(0x404040);
    Palette {
        bg: None,
        border: BorderType::Plain,
        border_color: Some(rgb(0x556D59)),
        title: Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(rgb(0xEEEEEE)),
        tab_active: Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(rgb(0xEEEEEE))
            .bg(rgb(0x6A2F2F)),
        tab_inactive: Style::default().fg(rgb(0x606060)),
        tab_separator: Some(rgb(0x303030)),
        bar_open: "  ",
        bar_close: "  ",
        filled_char: '\u{2588}',
        empty_char: ' ',
        empty_style: Style::default().fg(track).bg(track),
        bar_fill: btop_gradient,
        pct_color: btop_pct,
        kind: rgb(0x606060),
        status: rgb(0x606060),
        metrics: None,
        error: rgb(0xB54040),
        warn: rgb(0xDC4C4C),
        hint_key: Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(rgb(0xB54040)),
        hint_text: Some(rgb(0xCCCCCC)),
        timer_filled: Some(timer_btop),
        timer_empty: Some(rgb(0x404040)),
        loading: rgb(0x606060),
        ceiling_rpm: rgb(0x74E6FC),
        ceiling_in: rgb(0xB5E685),
        ceiling_out: rgb(0xFFD77A),
        welcome_title: Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(rgb(0xEEEEEE)),
    }
}

fn opencode_palette() -> Palette {
    let subtle = rgb(0x808080);
    let text = rgb(0xEEEEEE);
    Palette {
        bg: Some(rgb(0x0A0A0A)),
        border: BorderType::Rounded,
        border_color: Some(rgb(0x484848)),
        title: Style::default().add_modifier(Modifier::BOLD).fg(text),
        tab_active: Style::default().add_modifier(Modifier::BOLD).fg(text),
        tab_inactive: Style::default().fg(subtle),
        tab_separator: Some(rgb(0x3C3C3C)),
        bar_open: " [",
        bar_close: "] ",
        filled_char: '\u{28FF}',
        empty_char: '\u{28C0}',
        empty_style: Style::default().fg(rgb(0x3C3C3C)),
        bar_fill: opencode_bar_blue,
        pct_color: opencode_blue,
        kind: subtle,
        status: subtle,
        metrics: Some(text),
        error: rgb(0xE06C75),
        warn: rgb(0xF5A742),
        hint_key: Style::default().add_modifier(Modifier::BOLD).fg(text),
        hint_text: Some(subtle),
        timer_filled: Some(timer_opencode),
        timer_empty: Some(rgb(0x3C3C3C)),
        loading: subtle,
        ceiling_rpm: text,
        ceiling_in: text,
        ceiling_out: text,
        welcome_title: Style::default().add_modifier(Modifier::BOLD).fg(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_and_prev_cycle_through_all_themes() {
        assert_eq!(Theme::Default.next(), Theme::Crush);
        assert_eq!(Theme::Crush.next(), Theme::Btop);
        assert_eq!(Theme::Btop.next(), Theme::Opencode);
        assert_eq!(Theme::Opencode.next(), Theme::Default);
        assert_eq!(Theme::Default.prev(), Theme::Opencode);
        assert_eq!(Theme::Opencode.prev(), Theme::Btop);
        assert_eq!(Theme::Btop.prev(), Theme::Crush);
        assert_eq!(Theme::Crush.prev(), Theme::Default);
    }

    #[test]
    fn labels_are_stable() {
        assert_eq!(Theme::Default.label(), "default");
        assert_eq!(Theme::Crush.label(), "crush");
        assert_eq!(Theme::Btop.label(), "btop");
        assert_eq!(Theme::Opencode.label(), "opencode");
    }

    #[test]
    fn for_provider_maps_automated_themes() {
        assert_eq!(Theme::for_provider("Claude"), Theme::Default);
        assert_eq!(Theme::for_provider("Z.ai"), Theme::Opencode);
        assert_eq!(Theme::for_provider("OpenAI"), Theme::Opencode);
        assert_eq!(Theme::for_provider("Hyper"), Theme::Crush);
        assert_eq!(Theme::for_provider("Gemini"), Theme::Btop);
        assert_eq!(Theme::for_provider("Unknown"), Theme::Default);
    }

    #[test]
    fn default_palette_keeps_threshold_colors() {
        let p = Theme::Default.palette();
        assert_eq!((p.bar_fill)(10.0, 0.5), Color::Green);
        assert_eq!((p.bar_fill)(80.0, 0.5), Color::Yellow);
        assert_eq!((p.pct_color)(95.0), Color::Red);
    }

    #[test]
    fn btop_gradient_goes_green_to_red() {
        let p = Theme::Btop.palette();
        assert_eq!((p.bar_fill)(50.0, 0.0), rgb(0x77CA9B));
        assert_eq!((p.bar_fill)(50.0, 0.5), rgb(0xCBC06C));
        assert_eq!((p.bar_fill)(50.0, 1.0), rgb(0xDC4C4C));
        assert_eq!((p.pct_color)(0.0), rgb(0x77CA9B));
        assert_eq!((p.pct_color)(100.0), rgb(0xDC4C4C));
    }

    #[test]
    fn crush_gradient_goes_hazy_to_dolly() {
        let p = Theme::Crush.palette();
        assert_eq!((p.bar_fill)(50.0, 0.0), rgb(0x8B75FF));
        assert_eq!((p.bar_fill)(50.0, 1.0), rgb(0xFF60FF));
        assert_eq!(p.border_color, Some(rgb(0x8B75FF)));
    }

    #[test]
    fn crush_colors_stay_bright_on_dark_backgrounds() {
        let p = Theme::Crush.palette();
        for pos in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let Color::Rgb(r, g, b) = (p.bar_fill)(50.0, pos) else {
                panic!("expected rgb color");
            };
            assert!(
                r as u32 + g as u32 + b as u32 > 400,
                "gradient too dim at pos {pos}: #{r:02X}{g:02X}{b:02X}"
            );
        }
    }

    #[test]
    fn themes_with_solid_background() {
        assert_eq!(Theme::Default.palette().bg, None);
        assert_eq!(Theme::Crush.palette().bg, Some(rgb(0x1F1C23)));
        assert_eq!(Theme::Btop.palette().bg, None);
        assert_eq!(Theme::Opencode.palette().bg, Some(rgb(0x0A0A0A)));
    }

    #[test]
    fn opencode_bars_and_percentages_use_timer_blue() {
        let p = Theme::Opencode.palette();
        assert_eq!((p.bar_fill)(10.0, 0.0), rgb(0x5C9CF5));
        assert_eq!((p.bar_fill)(95.0, 1.0), rgb(0x5C9CF5));
        assert_eq!((p.pct_color)(95.0), rgb(0x5C9CF5));
        assert_eq!(
            (p.timer_filled.unwrap())(0.0),
            rgb(0x5C9CF5),
            "timer gradient starts at the same blue"
        );
    }

    #[test]
    fn opencode_uses_braille_dots_and_white_metrics() {
        let p = Theme::Opencode.palette();
        assert_eq!(p.filled_char, '\u{28FF}');
        assert_eq!(p.empty_char, '\u{28C0}');
        assert_eq!(p.metrics, Some(rgb(0xEEEEEE)));
        assert_eq!(
            p.title,
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(rgb(0xEEEEEE))
        );
        assert_eq!(p.tab_active, p.title);
        assert_eq!(
            (p.timer_filled.unwrap())(1.0),
            rgb(0x9D7CD8),
            "timer gradient keeps opencode accent purple"
        );
    }

    #[test]
    fn themes_use_distinct_border_and_bar_chars() {
        let d = Theme::Default.palette();
        let c = Theme::Crush.palette();
        let b = Theme::Btop.palette();
        let o = Theme::Opencode.palette();
        assert_ne!(d.filled_char, c.filled_char);
        assert_ne!(d.bar_open, b.bar_open);
        assert_ne!(d.tab_active, c.tab_active);
        assert_ne!(c.tab_active, b.tab_active);
        assert_ne!(o.filled_char, c.filled_char);
        assert_ne!(o.bar_open, b.bar_open);
        assert_ne!(o.tab_active, c.tab_active);
        assert_ne!(o.tab_active, b.tab_active);
    }
}
