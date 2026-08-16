//! The detail pane's renderer, and the empty state.
//!
//! The pane is immediate-mode, so there is nothing that remembers where a
//! control was drawn. Instead every frame interprets the node list
//! `app::detail::content()` produces — the same list `App::detail_items()`
//! derives the focusable items from — and records the cells each control
//! landed on. Because renderer and key handler consume one walk, item N on
//! screen *is* item N in the handler by construction; the recorded cells are
//! what a click resolves against.

use crate::app::detail::{ControlSpec, DetailNode, Field, ISSUES_HEADER};
use crate::app::{App, Focus, HitTarget, ScrollbarGeom};
use crate::theme;
use crate::ui::widgets::{TextInput, centered_rect};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

/// Width of the label column in the rename section.
const FIELD_LABEL_WIDTH: usize = 14;

/// Cells a card's border and padding take up on the left of its content, and
/// again on the right. Textual's cards were `border: solid` plus `padding: 1`.
const CARD_INSET: u16 = 2;

/// Cells left clear on the right of a card or a rule, from the `margin: 0 2 1 0`
/// Textual gave `.session-item` and `.issue-row` and the `margin: 1 2 1 0` it
/// gave `Rule.-horizontal`. Without it they run into the edge of the pane.
const RIGHT_MARGIN: u16 = 2;

/// Rows a control occupies, from Textual's `Button { border: solid; height: 3 }`:
/// its top border, its label, and its bottom border.
const CONTROL_HEIGHT: u16 = 3;

/// Columns the scrollbar occupies on the right of the pane, matching the two
/// Textual's scrollbar used.
const SCROLLBAR_WIDTH: u16 = 2;

/// `#detail-pane { padding: 1 2 }` — the pane's own inset from the divider.
const PANE_PAD_X: u16 = 2;
const PANE_PAD_Y: u16 = 1;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // `#detail-pane { padding: 1 2 }` — two columns each side and a blank row
    // at the top. The sidebar already draws the divider.
    let inner = Rect {
        x: area.x + PANE_PAD_X,
        y: area.y + PANE_PAD_Y,
        width: area.width.saturating_sub(2 * PANE_PAD_X),
        height: area.height.saturating_sub(PANE_PAD_Y),
    };

    let nodes = crate::app::detail::content(app);
    // Snapshot what this frame shows, enabled bits included: clicks and Enter
    // resolve against the frame the user saw, not a list a background event
    // may have reshaped since (see `App::drawn_items`).
    app.drawn_items = crate::app::detail::drawn(&nodes);
    // The pane can shrink under the cursor — the issue auto-refresh returning
    // fewer issues than last time is enough — and an index left past the end
    // resolves to nothing: the focus ring disappears and Enter does nothing
    // until the user presses Down. Clamped here because this is the one place
    // both consumers of the snapshot agree on its length.
    if app.detail_index >= app.drawn_items.len() {
        app.detail_index = app.drawn_items.len().saturating_sub(1);
    }
    if nodes.is_empty() {
        app.scrollbar = None;
        empty_state(frame, area);
        return;
    }
    let pane = render_nodes(app, nodes, inner.width);
    let offset = pane.resolve_offset(app, inner.height);
    let total = as_u16(pane.lines.len());
    frame.render_widget(Paragraph::new(pane.lines).scroll((offset, 0)), inner);

    // Published for the drag handler, which needs the same geometry the bar was
    // drawn with rather than its own copy of the arithmetic.
    app.scrollbar = if inner.width > SCROLLBAR_WIDTH {
        ScrollbarGeom::new(
            Rect {
                x: inner.x + inner.width - SCROLLBAR_WIDTH,
                y: inner.y,
                width: SCROLLBAR_WIDTH,
                height: inner.height,
            },
            total,
        )
    } else {
        None
    };
    draw_scrollbar(frame, app.scrollbar, offset);

    // The scroll offset decides which line ends up on which screen row, so this
    // is the earliest the clickable regions can be worked out.
    for (index, item) in pane.items.iter().enumerate() {
        // A control is three rows tall and so can straddle either edge of the
        // window. Clip it to what is on screen instead of dropping the region,
        // or a half-scrolled button would stop answering the mouse.
        let end = item.line.saturating_add(item.height);
        if end <= offset {
            continue;
        }
        let top = item.line.max(offset) - offset;
        if top >= inner.height {
            break;
        }
        app.push_hit(
            Rect {
                x: inner.x + item.x,
                y: inner.y + top,
                width: item.width,
                height: (end - offset - top).min(inner.height - top),
            },
            HitTarget::DetailItem(index),
        );
    }
}

/// Render the pane for the current selection, or `None` for the empty state.
/// Only tests call this; [`draw`] walks the content itself so it can snapshot
/// the drawn items first.
#[cfg(test)]
fn build(app: &App, width: u16) -> Option<Pane> {
    let nodes = crate::app::detail::content(app);
    if nodes.is_empty() {
        return None;
    }
    Some(render_nodes(app, nodes, width))
}

/// Interpret a node list into a laid-out pane.
fn render_nodes(app: &App, nodes: Vec<DetailNode>, width: u16) -> Pane {
    let focused = match app.focus {
        Focus::Detail => Some(app.detail_index),
        Focus::Sidebar => None,
    };
    // A hover only means something for the pane's own controls.
    let hovered = match app.hovered {
        Some(HitTarget::DetailItem(index)) => Some(index),
        _ => None,
    };
    let mut pane = Pane::new(focused, hovered, width);
    for node in nodes {
        render_node(&mut pane, app, node);
    }
    pane
}

/// Interpret one content node. This is the whole pane vocabulary; anything the
/// pane shows is one of these.
fn render_node(pane: &mut Pane, app: &App, node: DetailNode) {
    match node {
        DetailNode::Section(title) => pane.section(title),
        DetailNode::Text { text, style } => pane.text(text, style),
        DetailNode::Blank => pane.blank(),
        DetailNode::Rule => pane.rule(),
        DetailNode::PathBox { path, style } => path_box(pane, path, style),
        DetailNode::CardStart { padded } => {
            pane.card_start(pane.width.saturating_sub(RIGHT_MARGIN), padded);
        }
        DetailNode::CardEnd => pane.card_end(),
        DetailNode::Controls { lead, controls } => {
            let lead = lead
                .map(|(text, style)| vec![Span::styled(text, style)])
                .unwrap_or_default();
            pane.controls(lead, &controls);
        }
        DetailNode::Field { field, label } => {
            let input = match field {
                Field::WorktreeName => &app.name_input,
                Field::BranchName => &app.branch_input,
            };
            rename_field(pane, label, input);
        }
        DetailNode::IssuesHeader { glyph } => issues_header(pane, glyph),
    }
}

/// The issues header with its inline refresh control.
fn issues_header(pane: &mut Pane, glyph: char) {
    // `.refresh-btn { min-width: 3; width: 3 }` — the glyph is narrower than
    // the control, and it is the control that takes the click.
    let (focused, hovered) = pane.claim(as_u16(ISSUES_HEADER.chars().count() + 1), 3, 1);
    pane.row(vec![
        Span::styled(ISSUES_HEADER, theme::section_header()),
        Span::raw(" "),
        Span::styled(
            glyph.to_string(),
            if focused || hovered {
                theme::accent()
            } else {
                theme::secondary()
            },
        ),
    ]);
}

/// Where a focusable item landed. The line alone would not do: `Editor`,
/// `Terminal` and `Files` share one, so a click needs the x extent too.
struct Item {
    line: u16,
    x: u16,
    width: u16,
    /// Controls are [`CONTROL_HEIGHT`] rows; a rename field is one.
    height: u16,
}

/// Styles for a control's box: its border, then its label. The border carries
/// the state Textual gave it — accent under the cursor, the destructive shade
/// on a dangerous action — and the fill runs under the border cells too, which
/// is what `border: solid` did there.
fn control_styles(control: &ControlSpec, focused: bool, hovered: bool) -> (Style, Style) {
    // A control that cannot run must not light up under the pointer either:
    // an affordance that promises a click it will not honour is worse than
    // none. Textual disabled buttons the same way.
    let hovered = hovered && control.enabled;
    let fill = theme::action_bg(control.variant, hovered);
    let border = theme::action_border(focused, control.variant, hovered);
    // A control that cannot run keeps its box, so neither the layout nor the
    // hit region shifts; only the label goes grey — except under the cursor,
    // which has to stay legible.
    let label = if control.enabled || focused {
        theme::action(focused, control.variant, hovered)
    } else {
        theme::muted().bg(fill)
    };
    (border.bg(fill), label)
}

/// Rendered width of a control's box. Unlike the modal buttons, these have no
/// minimum width — the box hugs the label, which is how the pane laid out from
/// the start and what the committed sweep baselines show.
fn control_width(label: &str) -> u16 {
    crate::ui::widgets::boxed_width(label, 0)
}

/// The three rows of one control, drawn by the same box builder the modal
/// buttons use so the two cannot drift apart in shape.
fn control_box(control: &ControlSpec, focused: bool, hovered: bool) -> [Vec<Span<'static>>; 3] {
    let (border, label) = control_styles(control, focused, hovered);
    crate::ui::widgets::boxed_rows(&control.label, border, label, 0)
}

/// A card under construction — the bordered, elevated box Textual gave
/// `.session-item`, `.issue-row` and `.path-display`.
#[derive(Clone, Copy)]
struct Card {
    width: u16,
    /// Whether the card pads its content vertically. `.session-item` and
    /// `.issue-row` had `padding: 1`; `.path-display` padded only horizontally,
    /// so its box hugs the path.
    padded: bool,
}

/// Content under construction: the lines to render plus the cells each
/// focusable item claimed, in `App::detail_items()` order.
struct Pane {
    lines: Vec<Line<'static>>,
    items: Vec<Item>,
    /// Index of the focused item, or `None` while the sidebar owns focus.
    focused: Option<usize>,
    /// Index of the item the pointer is over, if any.
    hovered: Option<usize>,
    /// Width of the content area, so rules and cards can be filled out to it.
    width: u16,
    /// The card currently open, if any. See [`Pane::card_start`].
    card: Option<Card>,
}

impl Pane {
    fn new(focused: Option<usize>, hovered: Option<usize>, width: u16) -> Self {
        Self {
            lines: Vec::new(),
            items: Vec::new(),
            focused,
            hovered,
            width,
            card: None,
        }
    }

    fn text(&mut self, text: impl Into<String>, style: Style) {
        self.push_line(vec![Span::styled(text.into(), style)]);
    }

    /// A blank line — inside an open card, a *filled* one, so the card's
    /// background and borders carry through it rather than punching a hole.
    fn blank(&mut self) {
        self.push_line(Vec::new());
    }

    /// A section header. `.section-header { margin: 1 0 0 0 }` — the blank row
    /// above is unconditional, so the very first header is inset from the top of
    /// the pane exactly like every one after it.
    fn section(&mut self, title: &str) {
        self.blank();
        self.text(title.to_string(), theme::section_header());
    }

    /// The horizontal `Rule()` Textual drew between the major sections, with the
    /// blank line above it that its `margin` produced. The blank below is left to
    /// [`Pane::section`], which already spaces a header away from what precedes it.
    fn rule(&mut self) {
        self.blank();
        let width = self.width.saturating_sub(RIGHT_MARGIN);
        self.text("─".repeat(width as usize), theme::border());
    }

    /// Push a line built from spans returned by [`Pane::control`].
    fn row(&mut self, spans: Vec<Span<'static>>) {
        self.push_line(spans);
    }

    /// Open a card. Lines pushed until [`Pane::card_end`] land inside it.
    fn card_start(&mut self, width: u16, padded: bool) {
        self.lines.push(card_edge(width, '┌', '┐'));
        self.card = Some(Card { width, padded });
        if padded {
            self.blank();
        }
    }

    fn card_end(&mut self) {
        let Some(card) = self.card else {
            return;
        };
        if card.padded {
            self.blank();
        }
        self.card = None;
        self.lines.push(card_edge(card.width, '└', '┘'));
    }

    /// Push one content line, wrapped in the open card's border and background.
    fn push_line(&mut self, mut spans: Vec<Span<'static>>) {
        let Some(card) = self.card else {
            self.lines.push(Line::from(spans));
            return;
        };
        // One theme read for the whole line: consistent by construction, and
        // spares the lock a handful of reads per content row.
        let elevated = theme::active().bg_elevated;
        let width = card.width;
        let filled = Style::default().bg(elevated);
        // Text styles carry a foreground only, so without this the page colour
        // shows through the card. Spans that set their own background — the
        // controls — keep it, which is what makes one read as raised.
        for span in &mut spans {
            if span.style.bg.is_none() {
                span.style = span.style.bg(elevated);
            }
        }
        let content = as_u16(spans.iter().map(Span::width).sum());
        // Left border, left padding, content, then whatever is left over before
        // the padding and border on the right.
        let fill = width.saturating_sub(CARD_INSET + content + 1);
        let mut line = vec![
            Span::styled("│", theme::border().bg(elevated)),
            Span::styled(" ", filled),
        ];
        line.append(&mut spans);
        line.push(Span::styled(" ".repeat(fill as usize), filled));
        line.push(Span::styled("│", theme::border().bg(elevated)));
        self.lines.push(Line::from(line));
    }

    /// Claim the next focusable slot at `x`, `height` rows tall, starting on the
    /// line that is about to be pushed, and return whether it is the focused one.
    /// Callers must push those lines before adding any other, or the recorded
    /// cells drift from the content.
    fn claim(&mut self, x: u16, width: u16, height: u16) -> (bool, bool) {
        let focused = self.focused == Some(self.items.len());
        let hovered = self.hovered == Some(self.items.len());
        self.items.push(Item {
            line: as_u16(self.lines.len()),
            x,
            width,
            height,
        });
        (focused, hovered)
    }

    /// Lay out a row of controls after `lead`, which is whatever non-clickable
    /// text shares the line. Each control's extent is recorded as it is placed,
    /// because that running x offset is the only place it is ever known.
    ///
    /// The row costs three lines, since every control is a box: `lead` sits on
    /// the middle one, level with the labels.
    fn controls(&mut self, lead: Vec<Span<'static>>, controls: &[ControlSpec]) {
        // `.action-row { margin: 1 0 }` — a blank line above every row of
        // buttons. Inside a card the `padding: 1` already supplies it.
        let inset = match self.card {
            Some(_) => CARD_INSET,
            None => {
                self.blank();
                0
            }
        };
        let lead_width = as_u16(lead.iter().map(Span::width).sum());

        // `.session-buttons { align: right middle }` and `.issue-info { width:
        // 1fr }` both push a card's buttons against its right edge, one cell
        // clear of the border. Outside a card they stay left-aligned.
        let row_width = controls
            .iter()
            .map(|control| control_width(&control.label))
            .sum::<u16>()
            .saturating_add(as_u16(controls.len().saturating_sub(1)));
        let pad = self.card.map_or(0, |card| {
            card.width
                .saturating_sub(CARD_INSET + 2)
                .saturating_sub(lead_width.saturating_add(row_width))
        });

        let blank = |width: u16| Span::raw(" ".repeat(width as usize));
        let mut x = inset.saturating_add(lead_width).saturating_add(pad);
        let mut lead = lead;
        lead.push(blank(pad));
        let mut rows = [
            vec![blank(lead_width.saturating_add(pad))],
            lead,
            vec![blank(lead_width.saturating_add(pad))],
        ];

        for (index, control) in controls.iter().enumerate() {
            if index > 0 {
                for row in &mut rows {
                    row.push(blank(1));
                }
                x = x.saturating_add(1);
            }
            let width = control_width(&control.label);
            let (focused, hovered) = self.claim(x, width, CONTROL_HEIGHT);
            for (row, mut spans) in rows.iter_mut().zip(control_box(control, focused, hovered)) {
                row.append(&mut spans);
            }
            x = x.saturating_add(width);
        }

        for row in rows {
            self.push_line(row);
        }
    }

    /// The pane's scroll position for this frame.
    ///
    /// The offset lives on the app so the wheel can move it independently of the
    /// focus ring. Only content height — which is not known until the pane has
    /// been built — can clamp it, so the clamp happens here and is written back.
    fn resolve_offset(&self, app: &mut App, height: u16) -> u16 {
        let max = as_u16(self.lines.len()).saturating_sub(height);
        let mut offset = app.detail_scroll.min(max);

        // Keyboard navigation drags the viewport along so the cursor stays
        // visible. The wheel does not set this flag, which is what lets the user
        // scroll away from the focused control and read.
        if app.detail_follow_focus
            && let Some(item) = self.focused.and_then(|index| self.items.get(index))
        {
            let top = item.line;
            // Aim to clear the whole control plus two lines of context below it.
            let bottom = item.line.saturating_add(item.height).saturating_add(2);
            if top < offset {
                offset = top;
            } else if bottom > offset.saturating_add(height) {
                offset = bottom.saturating_sub(height);
            }
            offset = offset.min(max);
        }

        app.detail_follow_focus = false;
        app.detail_scroll = offset;
        offset
    }
}

/// The scrollbar on the right edge, drawn only when the content is taller than
/// the pane. Without one nothing on screen says the pane continues below the
/// fold.
///
/// Painted the way Textual painted it — two columns of background colour on
/// blank cells, rather than a glyph per row. A column of `│` would read as a
/// pane border, and it would also put a character on every row of the captured
/// text frames, burying real changes in the sweep diffs.
fn draw_scrollbar(frame: &mut Frame, geom: Option<ScrollbarGeom>, offset: u16) {
    let Some(geom) = geom else {
        return;
    };
    let top = geom.thumb_top(offset);
    let palette = theme::active();
    for row in 0..geom.track.height {
        let inside = row >= top && row < top.saturating_add(geom.thumb);
        let colour = if inside {
            palette.accent_dark
        } else {
            palette.scrollbar_trough
        };
        frame.render_widget(
            Block::default().style(Style::default().bg(colour)),
            Rect {
                y: geom.track.y + row,
                height: 1,
                ..geom.track
            },
        );
    }
}

/// Terminal geometry is `u16` while content lengths are `usize`.
fn as_u16(len: usize) -> u16 {
    u16::try_from(len).unwrap_or(u16::MAX)
}

/// Top or bottom edge of a card.
fn card_edge(width: u16, left: char, right: char) -> Line<'static> {
    let span = width.saturating_sub(2) as usize;
    Line::from(Span::styled(
        format!("{left}{}{right}", "─".repeat(span)),
        theme::border().bg(theme::active().bg_elevated),
    ))
}

/// A path in the boxed, elevated `.path-display` Textual used. The box hugs the
/// path rather than filling the pane, which is how it laid out there.
fn path_box(pane: &mut Pane, path: String, style: Style) {
    // A forest path routinely outruns the pane. Cutting it here rather than
    // letting the paragraph clip it keeps the box's right edge on screen; the
    // tail was lost either way.
    let room = pane.width.saturating_sub(2 * CARD_INSET) as usize;
    // `truncate` adds an ellipsis, so it needs three of those cells to itself.
    let path = if path.chars().count() > room {
        crate::util::truncate(&path, room.saturating_sub(3))
    } else {
        path
    };
    let width = as_u16(path.chars().count()).saturating_add(2 * CARD_INSET);
    pane.card_start(width, false);
    pane.text(path, style);
    pane.card_end();
}

/// A rename field. The caret is drawn in the text because the pane renders as
/// one scrolled paragraph and has no real terminal cursor to place.
fn rename_field(pane: &mut Pane, label: &str, input: &TextInput) {
    // Measured without consulting focus: the caret only ever occupies the one
    // trailing cell already counted here, so the click target is stable.
    let width = as_u16(FIELD_LABEL_WIDTH + 2 + input.value().chars().count());
    let (focused, _hovered) = pane.claim(0, width, 1);
    let mut spans = vec![Span::styled(
        format!("{label:<FIELD_LABEL_WIDTH$} "),
        theme::secondary(),
    )];

    if focused {
        let chars: Vec<char> = input.value().chars().collect();
        let cursor = input.cursor().min(chars.len());
        let before: String = chars[..cursor].iter().collect();
        let after: String = chars
            .get(cursor + 1..)
            .map(|rest| rest.iter().collect())
            .unwrap_or_default();
        // A cursor past the last character needs a cell of its own to sit in.
        let at = chars.get(cursor).copied().unwrap_or(' ');
        spans.push(Span::styled(before, theme::primary()));
        spans.push(Span::styled(at.to_string(), theme::cursor()));
        spans.push(Span::styled(after, theme::primary()));
    } else {
        spans.push(Span::styled(input.value().to_string(), theme::primary()));
    }
    pane.row(spans);
}

/// Nothing selected. The Textual version put this inside a zero-height
/// container, so it never appeared; centring it is what that build intended.
fn empty_state(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(" forestui", theme::accent())),
        Line::from(Span::styled("Git Worktree Manager", theme::secondary())),
        Line::default(),
        Line::from(Span::styled(
            "Select a repository or worktree",
            theme::muted(),
        )),
        Line::from(Span::styled(
            "or press [a] to add a repository",
            theme::muted(),
        )),
    ];
    let rect = centered_rect(area.width, as_u16(lines.len()), area);
    frame.render_widget(Paragraph::new(lines).centered(), rect);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, DetailItem, Field};
    use crate::event;
    use crate::models::{
        ClaudeSession, CustomClaudeButton, GitHubIssue, Repository, Settings, Worktree,
    };
    use crate::services::settings as settings_service;
    use crate::state::AppState;
    use chrono::Utc;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    /// An app whose state file lives in a throwaway forest directory.
    ///
    /// `App::new` reads the real settings and forest path, so both are pointed
    /// at temporary locations first and then replaced outright.
    fn test_app(dir: &tempfile::TempDir) -> App {
        settings_service::set_forest_path(dir.path().to_str());
        // Dropping the receiver is fine: sends are best-effort and no test reads
        // events back. Explicit default settings, never `App::new`: that reads
        // the developer's real ~/.config/forestui/settings.json and would
        // activate their personal theme, making test rendering machine-dependent.
        let (tx, _rx) = event::test_channel();
        let state = AppState::load_from(dir.path().join(".forestui-config.json"));
        App::with_state(tx, state, Settings::default())
    }

    fn with_repository(app: &mut App) {
        let repository = Repository::new("demo".into(), "/tmp/demo".into());
        let repo_id = repository.id;
        app.state.add_repository(repository);
        app.state.select_repository(repo_id);
    }

    fn with_worktree(app: &mut App) {
        let repository = Repository::new("demo".into(), "/tmp/demo".into());
        let repo_id = repository.id;
        app.state.add_repository(repository);
        let worktree = Worktree::new("wt".into(), "feat/x".into(), "/tmp/forest/demo/wt".into());
        let worktree_id = worktree.id;
        app.state.add_worktree(repo_id, worktree);
        app.state.select_worktree(repo_id, worktree_id);
    }

    fn a_session() -> ClaudeSession {
        ClaudeSession {
            id: "abc".into(),
            title: "Refactor the detail pane".into(),
            last_message: "and then some".into(),
            last_timestamp: Utc::now(),
            message_count: 12,
        }
    }

    fn an_issue() -> GitHubIssue {
        serde_json::from_str(
            r#"{"number":42,"title":"Fix login bug","state":"OPEN","url":"u",
                "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z",
                "author":{"login":"me"},"assignees":[],
                "labels":[{"name":"bug","color":"ff0000"}]}"#,
        )
        .expect("issue fixture parses")
    }

    /// One frame into a throwaway terminal. `crate::ui::draw` is what clears
    /// hits in the real app, so the test does it here.
    fn render_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
        app.clear_hits();
        terminal
            .draw(|frame| draw(frame, app, frame.area()))
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    /// Three-row controls and padded cards make the pane far taller than the
    /// terminal it usually runs in, so tests that want the whole thing on screen
    /// unscrolled have to ask for the room.
    const TALL: u16 = 80;

    fn render(app: &mut App) -> String {
        buffer_text(&render_buffer(app, 100, TALL))
    }

    /// Cell of the first occurrence of `needle`. Byte offsets would not do: the
    /// pill caps around every control are multi-byte.
    fn find_cell(buffer: &Buffer, needle: &str) -> (u16, u16) {
        let width = buffer.area.width as usize;
        for (y, row) in buffer.content.chunks(width).enumerate() {
            let line: String = row.iter().map(|cell| cell.symbol()).collect();
            if let Some(byte) = line.find(needle) {
                return (as_u16(line[..byte].chars().count()), as_u16(y));
            }
        }
        panic!("{needle:?} is not on screen");
    }

    fn detail_hits(app: &App) -> usize {
        app.hits
            .iter()
            .filter(|hit| matches!(hit.target, HitTarget::DetailItem(_)))
            .count()
    }

    fn buffer_text(buffer: &Buffer) -> String {
        let width = buffer.area.width as usize;
        buffer
            .content
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Textual put a `Rule()` between the major sections, and the pane read as
    /// one flat list without them.
    #[tokio::test]
    async fn sections_are_separated_by_rules() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_repository(&mut app);
        // Loaded but empty, so the whole pane fits on screen unscrolled.
        app.sessions = Some(vec![]);
        app.issues = Some(vec![]);

        /// Whether a rule sits between two lines. A rule is the only line that is
        /// nothing but the horizontal glyph; a card's edges start with a corner.
        fn rule_between(screen: &str, from: &str, to: &str) -> bool {
            let rows: Vec<&str> = screen.lines().collect();
            let row_of = |needle: &str| {
                rows.iter()
                    .position(|row| row.contains(needle))
                    .unwrap_or_else(|| panic!("{needle:?} is not on screen:\n{screen}"))
            };
            (row_of(from)..row_of(to)).any(|y| rows[y].trim().starts_with("──"))
        }

        let screen = render(&mut app);
        assert!(
            rule_between(&screen, "Repository: demo", "LOCATION"),
            "{screen}"
        );
        assert!(rule_between(&screen, "LOCATION", "OPEN IN"), "{screen}");
        assert!(rule_between(&screen, "OPEN IN", "CLAUDE"), "{screen}");
        assert!(
            rule_between(&screen, "RECENT SESSIONS", "MY OPEN GITHUB ISSUES"),
            "{screen}"
        );
        assert!(
            rule_between(&screen, "MY OPEN GITHUB ISSUES", "MANAGE"),
            "{screen}"
        );
        // CLAUDE ran straight into RECENT SESSIONS there, and does here too.
        assert!(
            !rule_between(&screen, "CLAUDE", "RECENT SESSIONS"),
            "{screen}"
        );

        let dir = tempfile::tempdir().expect("tempdir");
        app.state = AppState::load_from(dir.path().join(".forestui-config.json"));
        with_worktree(&mut app);
        let screen = render(&mut app);
        assert!(
            rule_between(&screen, "RECENT SESSIONS", "RENAME"),
            "{screen}"
        );
        assert!(rule_between(&screen, "Branch name", "MANAGE"), "{screen}");
    }

    /// Sessions and issues were `.session-item` / `.issue-row` boxes, not bare
    /// lines: a border in the border colour over an elevated background.
    #[tokio::test]
    async fn session_and_issue_items_render_as_cards() {
        // This test compares rendered colours against the active theme, so it
        // pins the theme for its whole render-and-assert window.
        let _guard = crate::theme::test_lock();
        crate::theme::set_active("forest-dark");
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_repository(&mut app);
        app.sessions = Some(vec![a_session()]);
        app.issues = Some(vec![an_issue()]);
        let buffer = render_buffer(&mut app, 100, TALL);
        // The pane is inset by its own padding and the card keeps a right margin
        // inside that, so the card's edge lands short of the screen edge.
        let right = buffer.area.width - PANE_PAD_X - RIGHT_MARGIN - 1;

        for label in ["Refactor the detail pane", "#42 Fix login bug"] {
            let (x, y) = find_cell(&buffer, label);
            let cell = |x: u16, y: u16| {
                buffer
                    .cell((x, y))
                    .unwrap_or_else(|| panic!("{label}: no cell at {x},{y}"))
            };

            let left = x - CARD_INSET;
            // `padding: 1` puts a blank card row between the top edge and the
            // first line of content.
            assert_eq!(cell(left, y - 2).symbol(), "┌", "{label}: top left");
            assert_eq!(cell(right, y - 2).symbol(), "┐", "{label}: top right");
            assert_eq!(cell(left, y - 1).symbol(), "│", "{label}: padding row");
            assert_eq!(
                cell(x, y - 1).bg,
                theme::active().bg_elevated,
                "{label}: padding row is filled",
            );
            assert_eq!(cell(left, y).symbol(), "│", "{label}: left border");
            assert_eq!(cell(right, y).symbol(), "│", "{label}: right border");
            assert_eq!(
                cell(left, y).fg,
                theme::active().border,
                "{label}: border colour"
            );
            // The background has to reach past the text, or the card is a frame
            // around the page colour rather than a filled box.
            for probe in [x, right - 1] {
                assert_eq!(
                    cell(probe, y).bg,
                    theme::active().bg_elevated,
                    "{label}: background at {probe}",
                );
            }
        }
    }

    #[tokio::test]
    async fn empty_state_is_visible() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        let screen = render(&mut app);

        // The Textual build laid this out into zero height and showed nothing.
        assert!(screen.contains("forestui"), "{screen}");
        assert!(screen.contains("Git Worktree Manager"), "{screen}");
        assert!(
            screen.contains("Select a repository or worktree"),
            "{screen}"
        );
        assert!(
            screen.contains("or press [a] to add a repository"),
            "{screen}"
        );
    }

    #[tokio::test]
    async fn repository_detail_renders_its_sections() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_repository(&mut app);
        let screen = render(&mut app);

        assert!(screen.contains("MAIN REPOSITORY"), "{screen}");
        assert!(screen.contains("Repository: demo"), "{screen}");
        assert!(screen.contains("LOCATION"), "{screen}");
        assert!(screen.contains("/tmp/demo"), "{screen}");
        assert!(screen.contains("CLAUDE"), "{screen}");
        assert!(screen.contains("New Session"), "{screen}");
    }

    #[tokio::test]
    async fn missing_worktree_directory_is_flagged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_worktree(&mut app);
        app.meta = event::DetailMeta {
            path: "/tmp/forest/demo/wt".into(),
            path_exists: false,
            ..event::DetailMeta::default()
        };
        let screen = render(&mut app);

        assert!(screen.contains("⚠ MISSING"), "{screen}");
        assert!(screen.contains("(missing)"), "{screen}");
        assert!(screen.contains("Git Pull (Directory missing)"), "{screen}");
    }

    /// The invariant: rendered control N is `detail_items()[N]`, so the counts
    /// must agree for every selection and every combination of loaded data.
    #[tokio::test]
    async fn rendered_controls_match_detail_items() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        app.settings.custom_buttons = vec![
            CustomClaudeButton {
                label: "Opus".into(),
                prefix: "opus".into(),
                command: "claude --model opus".into(),
            },
            CustomClaudeButton {
                label: "Disc".into(),
                prefix: "disc".into(),
                command: "claude --dangerously-skip-permissions".into(),
            },
        ];

        with_repository(&mut app);
        for sessions in [None, Some(vec![]), Some(vec![a_session(), a_session()])] {
            for issues in [None, Some(vec![]), Some(vec![an_issue()])] {
                app.sessions = sessions.clone();
                app.issues = issues.clone();
                let pane = build(&app, 100).expect("repository pane");
                assert_eq!(
                    pane.items.len(),
                    app.detail_items().len(),
                    "repository, {} sessions, {} issues",
                    app.sessions.as_ref().map_or(-1, |s| s.len() as i64),
                    app.issues.as_ref().map_or(-1, |i| i.len() as i64),
                );
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        app.state = AppState::load_from(dir.path().join(".forestui-config.json"));
        with_worktree(&mut app);
        for sessions in [None, Some(vec![]), Some(vec![a_session()])] {
            app.sessions = sessions.clone();
            let pane = build(&app, 100).expect("worktree pane");
            assert_eq!(
                pane.items.len(),
                app.detail_items().len(),
                "worktree, {} sessions",
                app.sessions.as_ref().map_or(-1, |s| s.len() as i64),
            );
        }
    }

    #[tokio::test]
    async fn focused_item_is_scrolled_into_view() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_repository(&mut app);
        app.sessions = Some(vec![a_session(), a_session(), a_session()]);
        app.issues = Some(vec![an_issue(), an_issue()]);
        app.focus = Focus::Detail;
        app.detail_index = app.detail_items().len() - 1;

        app.detail_follow_focus = true;
        let pane = build(&app, 100).expect("repository pane");
        let row = pane.items[app.detail_index].line;
        let offset = pane.resolve_offset(&mut app, 20);
        assert!(
            row >= offset && row < offset + 20,
            "row {row}, offset {offset}"
        );
        // Following is a one-shot: it must not fight the wheel on later frames.
        assert!(!app.detail_follow_focus);
    }

    /// `draw` is the only thing that ever writes `App::drawn_items`, and every
    /// activation — Enter and click alike — resolves against it. Delete that one
    /// line and the whole pane goes inert while every other test stays green,
    /// because they all snapshot it by hand.
    #[tokio::test]
    async fn drawing_the_pane_snapshots_what_it_drew() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_repository(&mut app);
        app.sessions = Some(vec![a_session()]);
        app.issues = Some(vec![an_issue()]);
        app.drawn_items.clear();

        render_buffer(&mut app, 100, TALL);

        assert!(!app.drawn_items.is_empty(), "the frame snapshotted nothing");
        assert_eq!(
            app.drawn_items,
            crate::app::detail::drawn(&crate::app::detail::content(&app)),
            "the snapshot is not what the frame drew"
        );
    }

    /// The pane shrinks under the cursor whenever the issue auto-refresh comes
    /// back with fewer issues than last time. An index left past the end is a
    /// focus ring that vanishes and an Enter that does nothing.
    #[tokio::test]
    async fn a_shrinking_pane_pulls_the_cursor_back_into_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_repository(&mut app);
        app.sessions = Some(vec![a_session()]);
        app.issues = Some(vec![an_issue(), an_issue(), an_issue()]);
        app.focus = Focus::Detail;
        app.detail_index = app.detail_items().len() - 1;
        render_buffer(&mut app, 100, TALL);

        // One issue was closed on GitHub since the last refresh.
        app.issues = Some(vec![an_issue()]);
        render_buffer(&mut app, 100, TALL);

        assert!(
            app.detail_index < app.drawn_items.len(),
            "index {} is past the end of {} items",
            app.detail_index,
            app.drawn_items.len()
        );
        assert!(
            app.drawn_items.get(app.detail_index).is_some(),
            "the focused slot resolves to nothing, so Enter is dead"
        );
    }

    /// The wheel scrolls the pane whoever has focus, which is the whole point of
    /// keeping the offset on the app rather than deriving it from the cursor.
    #[tokio::test]
    async fn the_pane_scrolls_while_the_sidebar_has_focus() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_repository(&mut app);
        app.sessions = Some(vec![a_session(), a_session(), a_session()]);
        app.issues = Some(vec![an_issue(), an_issue()]);
        app.focus = Focus::Sidebar;

        app.detail_scroll = 6;
        app.detail_follow_focus = false;
        let pane = build(&app, 100).expect("repository pane");
        assert_eq!(pane.resolve_offset(&mut app, 20), 6);
    }

    /// Scrolling past the end would leave the pane showing blank rows.
    #[tokio::test]
    async fn the_offset_is_clamped_to_the_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_repository(&mut app);
        app.focus = Focus::Sidebar;
        app.detail_scroll = u16::MAX;
        app.detail_follow_focus = false;

        let pane = build(&app, 100).expect("repository pane");
        let total = as_u16(pane.lines.len());
        let offset = pane.resolve_offset(&mut app, 20);
        assert_eq!(offset, total.saturating_sub(20));
        assert_eq!(app.detail_scroll, offset, "the clamp is written back");
    }

    /// Clicking a control has to reach the item drawn under the pointer, which
    /// is what the bug report said was broken.
    #[tokio::test]
    async fn clicking_a_control_maps_to_its_item() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_worktree(&mut app);
        app.sessions = Some(vec![a_session()]);
        let buffer = render_buffer(&mut app, 100, TALL);

        let index_of = |item: &DetailItem| {
            app.detail_items()
                .iter()
                .position(|candidate| candidate == item)
                .expect("item is rendered")
        };

        // Editor, Terminal and Files share a line, so getting these three right
        // is what proves the x extents are, and not just the rows.
        for (label, item) in [
            ("Editor", DetailItem::Action(Action::Editor)),
            ("Terminal", DetailItem::Action(Action::Terminal)),
            ("Files", DetailItem::Action(Action::Files)),
            ("Resume", DetailItem::Action(Action::ResumeSession(0))),
            ("Delete", DetailItem::Action(Action::Delete)),
            ("Worktree name", DetailItem::Field(Field::WorktreeName)),
        ] {
            let (x, y) = find_cell(&buffer, label);
            assert_eq!(
                app.hit_at(x, y),
                Some(HitTarget::DetailItem(index_of(&item))),
                "{label} at {x},{y}"
            );
        }
    }

    /// Textual drew every `Button` as a bordered box three rows tall, and its
    /// border is part of the button: clicking the top edge has to fire the same
    /// action as clicking the label under it.
    #[tokio::test]
    async fn a_control_is_three_rows_and_clickable_on_its_border() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_repository(&mut app);
        app.sessions = Some(vec![]);
        app.issues = Some(vec![]);
        let buffer = render_buffer(&mut app, 100, TALL);

        let (x, y) = find_cell(&buffer, "Add Worktree");
        let symbol = |x: u16, y: u16| {
            buffer
                .cell((x, y))
                .unwrap_or_else(|| panic!("no cell at {x},{y}"))
                .symbol()
                .to_string()
        };

        // The label sits behind the left border and its padding cell.
        let left = x - 2;
        assert_eq!(symbol(left, y - 1), "┌", "top left");
        assert_eq!(symbol(left, y), "│", "left border");
        assert_eq!(symbol(left, y + 1), "└", "bottom left");

        let target = app.hit_at(x, y);
        assert!(
            matches!(target, Some(HitTarget::DetailItem(_))),
            "the label is clickable: {target:?}"
        );
        assert_eq!(app.hit_at(x, y - 1), target, "top border row");
        assert_eq!(app.hit_at(x, y + 1), target, "bottom border row");
        // One row further out is a different item, or none at all.
        assert_ne!(app.hit_at(x, y + 2), target, "below the box");
    }

    #[tokio::test]
    async fn every_focusable_item_is_clickable_when_visible() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        app.settings.custom_buttons = vec![CustomClaudeButton {
            label: "Opus".into(),
            prefix: "opus".into(),
            command: "claude --model opus".into(),
        }];
        app.sessions = Some(vec![a_session()]);
        app.issues = Some(vec![an_issue()]);

        with_worktree(&mut app);
        // Tall and wide enough that nothing is scrolled or clipped away.
        render_buffer(&mut app, 120, TALL);
        assert_eq!(detail_hits(&app), app.detail_items().len(), "worktree");

        let dir = tempfile::tempdir().expect("tempdir");
        app.state = AppState::load_from(dir.path().join(".forestui-config.json"));
        with_repository(&mut app);
        render_buffer(&mut app, 120, TALL);
        assert_eq!(detail_hits(&app), app.detail_items().len(), "repository");
    }
}
