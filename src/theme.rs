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
/// Textual's `$bg-hover`, the fill every `:hover` rule in the stylesheet used.
pub const BG_HOVER: Color = Color::Rgb(0x3A, 0x3A, 0x3C);
/// Unfilled part of a scrollbar. Textual's default scrollbar track is darker
/// than the pane behind it, which is what makes the thumb readable.
pub const SCROLLBAR_TROUGH: Color = Color::Rgb(0x00, 0x00, 0x00);
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

/// Which of Textual's button variants a control carries.
///
/// `Primary` is `Button.-primary` — the accent pair the Textual build put on
/// "New Session" and every non-YOLO custom button. Without it those read as
/// ordinary buttons, which is a real difference: the green is how the safe
/// Claude action is told apart from the red one beside it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Variant {
    #[default]
    Normal,
    Primary,
    Destructive,
}

impl Variant {
    /// `-destructive` for a YOLO-style action, `-primary` otherwise — the split
    /// `repository_detail.py` made from `CustomClaudeButton.is_yolo_style`.
    pub fn claude(yolo: bool) -> Self {
        if yolo {
            Self::Destructive
        } else {
            Self::Primary
        }
    }

    pub fn is_destructive(self) -> bool {
        self == Self::Destructive
    }
}

/// `Button.-destructive { background: #3d2020 }`.
const DESTRUCTIVE_BG: Color = Color::Rgb(0x3D, 0x20, 0x20);
/// `Button.-destructive { border: solid #5a3030 }`.
const DESTRUCTIVE_BORDER: Color = Color::Rgb(0x5A, 0x30, 0x30);

/// `Button.-destructive:hover { background: #4d2828 }`.
const DESTRUCTIVE_BG_HOVER: Color = Color::Rgb(0x4D, 0x28, 0x28);

/// Background of a control ("button"), from its variant and whether the pointer
/// is over it.
///
/// Textual's `Button:focus` changed the *border* and nothing else, so focus must
/// not touch the fill here: tinting a focused plain button with the accent would
/// make it indistinguishable from a resting `-primary` one, which is the
/// difference between "the cursor is here" and "this is the safe action". Hover
/// does change the fill, because that is what `Button:hover` did.
pub fn action_bg(variant: Variant, hovered: bool) -> Color {
    match (variant, hovered) {
        // `Button:hover { background: $bg-hover }`
        (Variant::Normal, true) => BG_HOVER,
        (Variant::Normal, false) => BG_ELEVATED,
        // `Button.-primary:hover { background: $accent }`
        (Variant::Primary, true) => ACCENT,
        (Variant::Primary, false) => ACCENT_DARK,
        // `Button.-destructive:hover { background: #4d2828 }`
        (Variant::Destructive, true) => DESTRUCTIVE_BG_HOVER,
        (Variant::Destructive, false) => DESTRUCTIVE_BG,
    }
}

/// Style for an actionable item ("button") in the detail pane.
pub fn action(focused: bool, variant: Variant, hovered: bool) -> Style {
    let style = Style::default()
        .bg(action_bg(variant, hovered))
        .fg(match (variant, hovered) {
            // On `$accent` the light label would be near-invisible; Textual's
            // `-primary` text colour flips with the fill.
            (Variant::Primary, true) => BG,
            (v, _) if v.is_destructive() => DESTRUCTIVE,
            _ => TEXT_PRIMARY,
        });
    if focused {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

/// Border of a control's box. `Button:focus { border: solid $accent }` wins over
/// the variant's own colour, which is the only thing that marks the cursor.
/// `Button:hover` also sets an accent border, so a hovered control reads as
/// reachable even when the keyboard cursor is elsewhere.
pub fn action_border(focused: bool, variant: Variant, hovered: bool) -> Style {
    if focused {
        return Style::default().fg(ACCENT);
    }
    if hovered {
        // `Button.-destructive:hover { border: solid $destructive }`
        return Style::default().fg(if variant.is_destructive() {
            DESTRUCTIVE
        } else {
            ACCENT
        });
    }
    match variant {
        Variant::Normal => border(),
        Variant::Primary => Style::default().fg(ACCENT),
        Variant::Destructive => Style::default().fg(DESTRUCTIVE_BORDER),
    }
}
