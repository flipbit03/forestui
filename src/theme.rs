//! Colour palettes and shared styles.
//!
//! Every colour the UI paints lives on a [`Theme`], and exactly one theme is
//! active at a time. The default, Forest Dark, carries the hex values of the
//! Textual stylesheet unchanged, so the Rust build looks the way the Python
//! one did until the user picks something else in Settings.
//!
//! The active theme sits behind a `OnceLock<RwLock<..>>` (the same shape as
//! the runtime forest path in `services::settings`) rather than being threaded
//! through every renderer: ratatui redraws the whole frame each event and the
//! content walk re-runs per frame, so swapping the global repaints the entire
//! app on the very next frame with no invalidation step. That is also what
//! makes the theme picker's live preview a one-line operation.

use ratatui::style::{Color, Modifier, Style};
use std::sync::{OnceLock, RwLock};

/// One complete palette. Fields, not methods: renderers read
/// `theme::active().bg` the way they used to read the `BG` constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
    pub slug: &'static str,

    pub accent: Color,
    /// Fill of the selected row and of resting `-primary` buttons; must keep
    /// `text_primary` readable on top of it.
    pub accent_dark: Color,
    pub bg: Color,
    pub bg_elevated: Color,
    pub bg_selected: Color,
    /// Textual's `$bg-hover`, the fill every `:hover` rule used.
    pub bg_hover: Color,
    /// Unfilled part of a scrollbar, darker than the pane so the thumb reads.
    pub scrollbar_trough: Color,
    pub border: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub destructive: Color,
    pub success: Color,
    pub warning: Color,
    /// `Button.-destructive` fill / border / hover fill.
    pub destructive_bg: Color,
    pub destructive_border: Color,
    pub destructive_bg_hover: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb(
        ((hex >> 16) & 0xFF) as u8,
        ((hex >> 8) & 0xFF) as u8,
        (hex & 0xFF) as u8,
    )
}

/// Blend `pct_a` percent of `a` with the rest of `b`, per channel.
const fn mix(a: u32, b: u32, pct_a: u32) -> u32 {
    const fn channel(a: u32, b: u32, pct_a: u32) -> u32 {
        (a * pct_a + b * (100 - pct_a)) / 100
    }
    (channel((a >> 16) & 0xFF, (b >> 16) & 0xFF, pct_a) << 16)
        | (channel((a >> 8) & 0xFF, (b >> 8) & 0xFF, pct_a) << 8)
        | channel(a & 0xFF, b & 0xFF, pct_a)
}

/// Build a theme from the colours a published scheme actually names; the
/// blended tones every scheme leaves implicit (hover, the destructive button
/// fills, the accent row fill, the scrollbar trough) are derived by mixing, so
/// the palette table stays maintainable and internally consistent.
#[allow(clippy::too_many_arguments)]
const fn build(
    name: &'static str,
    slug: &'static str,
    bg: u32,
    bg_elevated: u32,
    bg_selected: u32,
    border: u32,
    text_primary: u32,
    text_secondary: u32,
    text_muted: u32,
    accent: u32,
    destructive: u32,
    warning: u32,
    success: u32,
) -> Theme {
    Theme {
        name,
        slug,
        accent: rgb(accent),
        accent_dark: rgb(mix(accent, bg, 45)),
        bg: rgb(bg),
        bg_elevated: rgb(bg_elevated),
        bg_selected: rgb(bg_selected),
        bg_hover: rgb(mix(bg_selected, bg_elevated, 50)),
        scrollbar_trough: rgb(mix(bg, 0x000000, 70)),
        border: rgb(border),
        text_primary: rgb(text_primary),
        text_secondary: rgb(text_secondary),
        text_muted: rgb(text_muted),
        destructive: rgb(destructive),
        success: rgb(success),
        warning: rgb(warning),
        destructive_bg: rgb(mix(destructive, bg, 25)),
        destructive_border: rgb(mix(destructive, bg, 40)),
        destructive_bg_hover: rgb(mix(destructive, bg, 32)),
    }
}

/// The palette the Textual build shipped, verbatim — including its hand-picked
/// blends, which is why it does not go through [`build`].
const FOREST_DARK: Theme = Theme {
    name: "Forest Dark",
    slug: "forest-dark",
    accent: rgb(0x52B788),
    accent_dark: rgb(0x2D6A4F),
    bg: rgb(0x1C1C1E),
    bg_elevated: rgb(0x2C2C2E),
    bg_selected: rgb(0x48484A),
    bg_hover: rgb(0x3A3A3C),
    scrollbar_trough: rgb(0x000000),
    border: rgb(0x3D3D3F),
    text_primary: rgb(0xF5F5F5),
    text_secondary: rgb(0xA8A8A8),
    text_muted: rgb(0x7A7A7A),
    destructive: rgb(0xFF6B6B),
    success: rgb(0x52B788),
    warning: rgb(0xFFB347),
    destructive_bg: rgb(0x3D2020),
    destructive_border: rgb(0x5A3030),
    destructive_bg_hover: rgb(0x4D2828),
};

/// Every selectable theme, default first. Palettes use each scheme's published
/// canonical values.
#[rustfmt::skip]
pub const THEMES: &[Theme] = &[
    FOREST_DARK,
    //    name                 slug                   bg        elevated  selected  border    text      secondary muted     accent    destruct  warning   success
    build("Dracula",           "dracula",             0x282A36, 0x313342, 0x44475A, 0x44475A, 0xF8F8F2, 0xBFC7D5, 0x6272A4, 0xBD93F9, 0xFF5555, 0xFFB86C, 0x50FA7B),
    build("Nord",              "nord",                0x2E3440, 0x3B4252, 0x434C5E, 0x4C566A, 0xECEFF4, 0xD8DEE9, 0x7B88A1, 0x88C0D0, 0xBF616A, 0xEBCB8B, 0xA3BE8C),
    build("Gruvbox Dark",      "gruvbox-dark",        0x282828, 0x3C3836, 0x504945, 0x504945, 0xEBDBB2, 0xD5C4A1, 0x928374, 0xB8BB26, 0xFB4934, 0xFABD2F, 0xB8BB26),
    build("Gruvbox Light",     "gruvbox-light",       0xFBF1C7, 0xEBDBB2, 0xD5C4A1, 0xD5C4A1, 0x3C3836, 0x504945, 0x928374, 0x79740E, 0x9D0006, 0xB57614, 0x79740E),
    build("Solarized Dark",    "solarized-dark",      0x002B36, 0x073642, 0x586E75, 0x586E75, 0xFDF6E3, 0x93A1A1, 0x657B83, 0x2AA198, 0xDC322F, 0xB58900, 0x859900),
    build("Solarized Light",   "solarized-light",     0xFDF6E3, 0xEEE8D5, 0xD9D2C0, 0x93A1A1, 0x073642, 0x586E75, 0x93A1A1, 0x2AA198, 0xDC322F, 0xB58900, 0x859900),
    build("Catppuccin Mocha",  "catppuccin-mocha",    0x1E1E2E, 0x313244, 0x45475A, 0x45475A, 0xCDD6F4, 0xBAC2DE, 0x6C7086, 0xCBA6F7, 0xF38BA8, 0xF9E2AF, 0xA6E3A1),
    build("Catppuccin Macchiato", "catppuccin-macchiato", 0x24273A, 0x363A4F, 0x494D64, 0x494D64, 0xCAD3F5, 0xB8C0E0, 0x6E738D, 0xC6A0F6, 0xED8796, 0xEED49F, 0xA6DA95),
    build("Catppuccin Frappé", "catppuccin-frappe",   0x303446, 0x414559, 0x51576D, 0x51576D, 0xC6D0F5, 0xB5BFE2, 0x737994, 0xCA9EE6, 0xE78284, 0xE5C890, 0xA6D189),
    build("Catppuccin Latte",  "catppuccin-latte",    0xEFF1F5, 0xE6E9EF, 0xCCD0DA, 0xBCC0CC, 0x4C4F69, 0x5C5F77, 0x8C8FA1, 0x8839EF, 0xD20F39, 0xDF8E1D, 0x40A02B),
    build("Tokyo Night",       "tokyo-night",         0x1A1B26, 0x24283B, 0x33467C, 0x3B4261, 0xC0CAF5, 0xA9B1D6, 0x565F89, 0x7AA2F7, 0xF7768E, 0xE0AF68, 0x9ECE6A),
    build("Tokyo Night Storm", "tokyo-night-storm",   0x24283B, 0x292E42, 0x3B4261, 0x414868, 0xC0CAF5, 0xA9B1D6, 0x565F89, 0x7AA2F7, 0xF7768E, 0xE0AF68, 0x9ECE6A),
    build("Tokyo Night Day",   "tokyo-night-day",     0xE1E2E7, 0xD5D6DB, 0xC4C8DA, 0xA8AECB, 0x3760BF, 0x6172B0, 0x848CB5, 0x2E7DE9, 0xF52A65, 0x8C6C3E, 0x587539),
    build("One Dark",          "one-dark",            0x282C34, 0x2C313A, 0x3E4451, 0x3E4451, 0xDCDFE4, 0xABB2BF, 0x5C6370, 0x61AFEF, 0xE06C75, 0xE5C07B, 0x98C379),
    build("Monokai",           "monokai",             0x272822, 0x3E3D32, 0x49483E, 0x49483E, 0xF8F8F2, 0xCFCFC2, 0x75715E, 0xA6E22E, 0xF92672, 0xFD971F, 0xA6E22E),
    build("Rosé Pine",         "rose-pine",           0x191724, 0x1F1D2E, 0x403D52, 0x403D52, 0xE0DEF4, 0x908CAA, 0x6E6A86, 0xC4A7E7, 0xEB6F92, 0xF6C177, 0x9CCFD8),
    build("Rosé Pine Moon",    "rose-pine-moon",      0x232136, 0x2A273F, 0x44415A, 0x44415A, 0xE0DEF4, 0x908CAA, 0x6E6A86, 0xC4A7E7, 0xEB6F92, 0xF6C177, 0x9CCFD8),
    build("Rosé Pine Dawn",    "rose-pine-dawn",      0xFAF4ED, 0xF2E9E1, 0xDFDAD9, 0xCECACD, 0x575279, 0x797593, 0x9893A5, 0x907AA9, 0xB4637A, 0xEA9D34, 0x56949F),
    build("Ayu Dark",          "ayu-dark",            0x0A0E14, 0x131721, 0x253340, 0x253340, 0xB3B1AD, 0x9A9793, 0x626A73, 0xE6B450, 0xF26D78, 0xFFB454, 0xC2D94C),
    build("Ayu Mirage",        "ayu-mirage",          0x1F2430, 0x232834, 0x33415E, 0x33415E, 0xCBCCC6, 0xB1B5AF, 0x5C6773, 0xFFCC66, 0xF28779, 0xFFA759, 0xBAE67E),
    build("Ayu Light",         "ayu-light",           0xFAFAFA, 0xF3F4F5, 0xD2EBFF, 0xD9D8D7, 0x5C6166, 0x787B80, 0xA0A6AC, 0xFF9940, 0xE65050, 0xF2AE49, 0x86B300),
    build("Everforest",        "everforest",          0x2D353B, 0x343F44, 0x475258, 0x475258, 0xD3C6AA, 0x9DA9A0, 0x859289, 0xA7C080, 0xE67E80, 0xDBBC7F, 0xA7C080),
    build("Kanagawa",          "kanagawa",            0x1F1F28, 0x2A2A37, 0x363646, 0x54546D, 0xDCD7BA, 0xC8C093, 0x727169, 0x7E9CD8, 0xC34043, 0xC0A36E, 0x98BB6C),
    build("Nightfox",          "nightfox",            0x192330, 0x212E3F, 0x29394F, 0x39506D, 0xCDCECF, 0xB6BDCA, 0x71839B, 0x719CD6, 0xC94F6D, 0xDBC074, 0x81B29A),
    build("Iceberg",           "iceberg",             0x161821, 0x1E2132, 0x272C42, 0x6B7089, 0xC6C8D1, 0xA3ADCB, 0x6B7089, 0x84A0C6, 0xE27878, 0xE2A478, 0xB4BE82),
    build("Zenburn",           "zenburn",             0x3F3F3F, 0x4F4F4F, 0x5F5F5F, 0x5F5F5F, 0xDCDCCC, 0xD0D0B8, 0x9F9F8F, 0x8CD0D3, 0xCC9393, 0xF0DFAF, 0x7F9F7F),
    build("GitHub Dark",       "github-dark",         0x0D1117, 0x161B22, 0x21262D, 0x30363D, 0xE6EDF3, 0xC9D1D9, 0x6E7681, 0x58A6FF, 0xF85149, 0xD29922, 0x3FB950),
    build("GitHub Light",      "github-light",        0xFFFFFF, 0xF6F8FA, 0xEAEEF2, 0xD0D7DE, 0x24292F, 0x57606A, 0x6E7781, 0x0969DA, 0xCF222E, 0x9A6700, 0x1A7F37),
    build("Horizon",           "horizon",             0x1C1E26, 0x232530, 0x2E303E, 0x2E303E, 0xCBCED0, 0x9DA0A2, 0x6C6F93, 0x25B0BC, 0xE95678, 0xFAB795, 0x29D398),
    build("Palenight",         "palenight",           0x292D3E, 0x32374D, 0x444267, 0x444267, 0xEEFFFF, 0xA6ACCD, 0x676E95, 0xC792EA, 0xF07178, 0xFFCB6B, 0xC3E88D),
    build("Night Owl",         "night-owl",           0x011627, 0x0B2942, 0x1D3B53, 0x5F7E97, 0xD6DEEB, 0xA2B5CB, 0x637777, 0x82AAFF, 0xEF5350, 0xECC48D, 0xADDB67),
    // SynthWave '84 is mapped from robb0wen/synthwave-vscode's theme JSON:
    // editor.background, input.background (widgets sit on a *darker* surface
    // there), selection/ruler colours composited over the background where the
    // original uses alpha, the Comment lavender as muted, and the signature
    // pink/red/yellow/green token colours.
    build("SynthWave '84",     "synthwave-84",        0x262335, 0x2A2139, 0x413E4E, 0x633670, 0xFFFFFF, 0xB6B1B1, 0x848BBD, 0xFF7EDB, 0xFE4450, 0xFEDE5D, 0x72F1B8),
];

/// Look a theme up by its settings slug.
pub fn by_slug(slug: &str) -> Option<&'static Theme> {
    THEMES.iter().find(|t| t.slug == slug)
}

fn active_cell() -> &'static RwLock<&'static Theme> {
    static ACTIVE: OnceLock<RwLock<&'static Theme>> = OnceLock::new();
    ACTIVE.get_or_init(|| RwLock::new(&THEMES[0]))
}

/// The palette every renderer reads. Swapped whole; never partially updated.
pub fn active() -> &'static Theme {
    active_cell()
        .read()
        .map(|guard| *guard)
        .unwrap_or(&THEMES[0])
}

/// Activate the theme a settings slug names. Unknown or legacy values (the
/// old "system" / "dark" / "light" strings older settings files carry) fall
/// back to the default palette — a stored slug must never block startup or
/// change the schema's meaning.
pub fn set_active(slug: &str) {
    // The active theme is process-global, and `cargo test` runs threads in
    // parallel: every mutation serializes behind the test lock so a picker
    // test's preview window cannot be clobbered by another test constructing
    // an App mid-assertion. Re-entrant per thread, so a test already holding
    // the lock can call this freely. Serialize-only — the restoring guard
    // here would undo this very call on return. Compiled out of real builds.
    #[cfg(test)]
    let _guard = test_sync::serialize_only();
    let theme = by_slug(slug).unwrap_or(&THEMES[0]);
    if let Ok(mut guard) = active_cell().write() {
        *guard = theme;
    }
}

pub fn primary() -> Style {
    Style::default().fg(active().text_primary)
}

pub fn secondary() -> Style {
    Style::default().fg(active().text_secondary)
}

pub fn muted() -> Style {
    Style::default().fg(active().text_muted)
}

pub fn accent() -> Style {
    Style::default().fg(active().accent)
}

pub fn destructive() -> Style {
    Style::default().fg(active().destructive)
}

pub fn section_header() -> Style {
    Style::default()
        .fg(active().text_secondary)
        .add_modifier(Modifier::BOLD)
}

pub fn title() -> Style {
    Style::default()
        .fg(active().text_primary)
        .add_modifier(Modifier::BOLD)
}

pub fn border() -> Style {
    Style::default().fg(active().border)
}

pub fn border_focused() -> Style {
    Style::default().fg(active().accent)
}

/// Style for the highlighted row of a list.
pub fn cursor() -> Style {
    Style::default()
        .bg(active().accent_dark)
        .fg(active().text_primary)
}

/// Style for the highlighted row when the list does not have focus.
pub fn cursor_unfocused() -> Style {
    Style::default()
        .bg(active().bg_selected)
        .fg(active().text_primary)
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

/// Background of a control ("button"), from its variant and whether the pointer
/// is over it.
///
/// Textual's `Button:focus` changed the *border* and nothing else, so focus must
/// not touch the fill here: tinting a focused plain button with the accent would
/// make it indistinguishable from a resting `-primary` one, which is the
/// difference between "the cursor is here" and "this is the safe action". Hover
/// does change the fill, because that is what `Button:hover` did.
pub fn action_bg(variant: Variant, hovered: bool) -> Color {
    let theme = active();
    match (variant, hovered) {
        // `Button:hover { background: $bg-hover }`
        (Variant::Normal, true) => theme.bg_hover,
        (Variant::Normal, false) => theme.bg_elevated,
        // `Button.-primary:hover { background: $accent }`
        (Variant::Primary, true) => theme.accent,
        (Variant::Primary, false) => theme.accent_dark,
        // `Button.-destructive:hover { background: #4d2828 }`
        (Variant::Destructive, true) => theme.destructive_bg_hover,
        (Variant::Destructive, false) => theme.destructive_bg,
    }
}

/// Style for an actionable item ("button") in the detail pane.
pub fn action(focused: bool, variant: Variant, hovered: bool) -> Style {
    let theme = active();
    let style = Style::default()
        .bg(action_bg(variant, hovered))
        .fg(match (variant, hovered) {
            // On `$accent` the light label would be near-invisible; Textual's
            // `-primary` text colour flips with the fill.
            (Variant::Primary, true) => theme.bg,
            (v, _) if v.is_destructive() => theme.destructive,
            _ => theme.text_primary,
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
    let theme = active();
    if focused {
        return Style::default().fg(theme.accent);
    }
    if hovered {
        // `Button.-destructive:hover { border: solid $destructive }`
        return Style::default().fg(if variant.is_destructive() {
            theme.destructive
        } else {
            theme.accent
        });
    }
    match variant {
        Variant::Normal => border(),
        Variant::Primary => Style::default().fg(theme.accent),
        Variant::Destructive => Style::default().fg(theme.destructive_border),
    }
}

/// Serializes tests that touch the process-global active theme, so a preview
/// in one test cannot race an assertion (or an `App` construction) in another.
/// Re-entrant per thread: `set_active` takes it internally, and a test already
/// holding it gets a no-op guard instead of deadlocking itself. On drop the
/// outermost guard restores whatever theme was active when it was taken —
/// cleanup that a panic cannot skip and no test has to remember.
#[cfg(test)]
pub(crate) fn test_lock() -> test_sync::ThemeGuard {
    test_sync::lock()
}

#[cfg(test)]
pub(crate) mod test_sync {
    use std::cell::Cell;
    use std::sync::{Mutex, MutexGuard};

    static LOCK: Mutex<()> = Mutex::new(());
    thread_local! {
        static HELD: Cell<bool> = const { Cell::new(false) };
    }

    pub struct ThemeGuard {
        /// The lock itself, plus the slug to put back — `None` for re-entrant
        /// and serialize-only guards, which own neither.
        held: Option<(MutexGuard<'static, ()>, Option<&'static str>)>,
    }

    impl Drop for ThemeGuard {
        fn drop(&mut self) {
            if let Some((_lock, restore)) = self.held.take() {
                if let Some(slug) = restore {
                    // HELD is still set, so this re-enters as a no-op guard.
                    super::set_active(slug);
                }
                HELD.set(false);
                // `_lock` releases here, after the restore.
            }
        }
    }

    /// The restoring flavour, for tests.
    pub fn lock() -> ThemeGuard {
        acquire(true)
    }

    /// Serialization without restore-on-drop, for `set_active` itself.
    pub(crate) fn serialize_only() -> ThemeGuard {
        acquire(false)
    }

    fn acquire(restore: bool) -> ThemeGuard {
        if HELD.get() {
            // This thread already owns the lock; hand out a no-op guard.
            return ThemeGuard { held: None };
        }
        let guard = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        HELD.set(true);
        let previous = restore.then(|| super::active().slug);
        ThemeGuard {
            held: Some((guard, previous)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The settings file stores the slug; a collision would make two themes
    /// indistinguishable on disk, and an empty slug unselectable.
    #[test]
    fn every_theme_has_a_unique_slug() {
        let mut seen = HashSet::new();
        for theme in THEMES {
            assert!(!theme.slug.is_empty() && !theme.name.is_empty());
            assert!(
                theme
                    .slug
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "slug '{}' is not kebab-case",
                theme.slug
            );
            assert!(seen.insert(theme.slug), "duplicate slug '{}'", theme.slug);
        }
    }

    #[test]
    fn the_default_theme_keeps_the_textual_palette() {
        assert_eq!(THEMES[0].slug, "forest-dark");
        assert_eq!(THEMES[0].bg, Color::Rgb(0x1C, 0x1C, 0x1E));
        assert_eq!(THEMES[0].accent, Color::Rgb(0x52, 0xB7, 0x88));
        assert_eq!(THEMES[0].destructive_bg, Color::Rgb(0x3D, 0x20, 0x20));
    }

    #[test]
    fn unknown_and_legacy_slugs_fall_back_to_the_default() {
        let _guard = test_lock();
        assert!(by_slug("forest-dark").is_some());
        assert!(by_slug("system").is_none());
        assert!(by_slug("dark").is_none());
        // set_active with garbage must leave a usable palette.
        set_active("definitely-not-a-theme");
        assert_eq!(active().slug, "forest-dark");
    }

    /// Each theme's text must be readable on its own backgrounds: a crude
    /// luminance-distance check, but it catches a light-on-light or
    /// dark-on-dark palette typo the moment the table grows.
    #[test]
    fn text_contrasts_with_backgrounds_in_every_theme() {
        fn luma(color: Color) -> i32 {
            let Color::Rgb(r, g, b) = color else {
                panic!("themes are all Rgb");
            };
            (r as i32 * 299 + g as i32 * 587 + b as i32 * 114) / 1000
        }
        for theme in THEMES {
            for (label, bg) in [
                ("bg", theme.bg),
                ("bg_elevated", theme.bg_elevated),
                ("accent_dark", theme.accent_dark),
            ] {
                let distance = (luma(theme.text_primary) - luma(bg)).abs();
                assert!(
                    distance >= 60,
                    "{}: text_primary vs {label} distance {distance}",
                    theme.slug
                );
            }
        }
    }
}
