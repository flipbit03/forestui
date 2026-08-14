//! Colour palette and shared styles.
//!
//! The hex values are carried over unchanged from the Textual stylesheet so the
//! Rust build looks the same as the Python one did.

use ratatui::style::{Color, Modifier, Style};

pub const ACCENT: Color = Color::Rgb(0x52, 0xB7, 0x88);
pub const ACCENT_DARK: Color = Color::Rgb(0x2D, 0x6A, 0x4F);
pub const BG: Color = Color::Rgb(0x1C, 0x1C, 0x1E);
pub const BG_ELEVATED: Color = Color::Rgb(0x2C, 0x2C, 0x2E);
pub const BG_SELECTED: Color = Color::Rgb(0x48, 0x48, 0x4A);
pub const BORDER: Color = Color::Rgb(0x3D, 0x3D, 0x3F);
pub const TEXT_PRIMARY: Color = Color::Rgb(0xF5, 0xF5, 0xF5);
pub const TEXT_SECONDARY: Color = Color::Rgb(0xA8, 0xA8, 0xA8);
pub const TEXT_MUTED: Color = Color::Rgb(0x7A, 0x7A, 0x7A);
pub const DESTRUCTIVE: Color = Color::Rgb(0xFF, 0x6B, 0x6B);
pub const SUCCESS: Color = Color::Rgb(0x52, 0xB7, 0x88);
pub const WARNING: Color = Color::Rgb(0xFF, 0xB3, 0x47);

pub fn primary() -> Style {
    Style::default().fg(TEXT_PRIMARY)
}

pub fn secondary() -> Style {
    Style::default().fg(TEXT_SECONDARY)
}

pub fn muted() -> Style {
    Style::default().fg(TEXT_MUTED)
}

pub fn accent() -> Style {
    Style::default().fg(ACCENT)
}

pub fn destructive() -> Style {
    Style::default().fg(DESTRUCTIVE)
}

pub fn section_header() -> Style {
    Style::default()
        .fg(TEXT_SECONDARY)
        .add_modifier(Modifier::BOLD)
}

pub fn title() -> Style {
    Style::default()
        .fg(TEXT_PRIMARY)
        .add_modifier(Modifier::BOLD)
}

pub fn border() -> Style {
    Style::default().fg(BORDER)
}

pub fn border_focused() -> Style {
    Style::default().fg(ACCENT)
}

/// Style for the highlighted row of a list.
pub fn cursor() -> Style {
    Style::default().bg(ACCENT_DARK).fg(TEXT_PRIMARY)
}

/// Style for the highlighted row when the list does not have focus.
pub fn cursor_unfocused() -> Style {
    Style::default().bg(BG_SELECTED).fg(TEXT_PRIMARY)
}

/// Background of a control ("button"), also used to draw its rounded caps.
///
/// The unfocused destructive shade matches the Textual build's
/// `Button.-destructive` background, so a Delete button still reads as dangerous
/// when the cursor is elsewhere.
pub fn action_bg(focused: bool, destructive_action: bool) -> Color {
    match (focused, destructive_action) {
        (true, true) => Color::Rgb(0x4D, 0x28, 0x28),
        (true, false) => ACCENT_DARK,
        (false, true) => Color::Rgb(0x3D, 0x20, 0x20),
        (false, false) => BG_ELEVATED,
    }
}

/// Style for an actionable item ("button") in the detail pane.
pub fn action(focused: bool, destructive_action: bool) -> Style {
    let style = Style::default()
        .bg(action_bg(focused, destructive_action))
        .fg(if destructive_action {
            DESTRUCTIVE
        } else {
            TEXT_PRIMARY
        });
    if focused {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}
