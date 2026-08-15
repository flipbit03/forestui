//! Mouse input: hit testing, hover, wheel routing, and the scrollbar.
//!
//! Immediate mode keeps no widget tree, so there is nothing to ask "what is at
//! this cell?". Each frame the renderers record the rectangle of every
//! clickable thing via [`App::push_hit`], and clicks are resolved against that
//! list — last recorded wins, so a modal takes clicks from the panes it
//! covers.

use super::{App, DetailItem, Focus, SCROLL_STEP, SidebarRow};
use crate::event::AppEvent;

/// Pointer motion, held button or not. Either may well leave the frame
/// identical, so both are left to the handler to mark dirty when it acts.
pub(super) fn is_pointer_motion(event: &AppEvent) -> bool {
    use ratatui::crossterm::event::{Event, MouseEventKind};
    matches!(
        event,
        AppEvent::Term(Event::Mouse(mouse))
            if matches!(mouse.kind, MouseEventKind::Moved | MouseEventKind::Drag(_))
    )
}

/// Where the detail pane's scrollbar was drawn, and the arithmetic that maps
/// between a thumb position and a scroll offset.
///
/// Kept in one place because the renderer and the drag handler have to agree
/// exactly: a thumb that renders one row off from where the drag thinks it is
/// feels like the bar slipping out from under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarGeom {
    /// The full track, including the part the thumb is not covering.
    pub track: ratatui::layout::Rect,
    pub thumb: u16,
    /// Rows the top of the thumb can travel.
    pub travel: u16,
    pub max_offset: u16,
}

impl ScrollbarGeom {
    /// `None` when the content fits, which is also when no bar is drawn.
    pub fn new(track: ratatui::layout::Rect, total: u16) -> Option<Self> {
        let height = track.height;
        if height == 0 || total <= height {
            return None;
        }
        // At least one cell, so the thumb never vanishes on very long content.
        let thumb = ((u32::from(height) * u32::from(height)) / u32::from(total)).max(1) as u16;
        Some(Self {
            track,
            thumb,
            travel: height.saturating_sub(thumb),
            max_offset: total - height,
        })
    }

    /// Rounds to nearest rather than truncating. There are usually far more
    /// scroll positions than track rows, so truncating leaves the thumb a row
    /// behind the offset a drag just produced — the bar visibly lags the pointer
    /// and never quite reaches the bottom. Rounding makes this the exact inverse
    /// of [`Self::offset_at`] for every row of the track.
    pub fn thumb_top(&self, offset: u16) -> u16 {
        if self.max_offset == 0 {
            return 0;
        }
        let max = u32::from(self.max_offset);
        let scaled = u32::from(offset.min(self.max_offset)) * u32::from(self.travel);
        (((scaled + max / 2) / max) as u16).min(self.travel)
    }

    pub fn offset_at(&self, thumb_top: u16) -> u16 {
        if self.travel == 0 {
            return 0;
        }
        ((u32::from(thumb_top.min(self.travel)) * u32::from(self.max_offset))
            / u32::from(self.travel)) as u16
    }
}

/// Is this cell inside that rectangle?
fn contains(rect: ratatui::layout::Rect, column: u16, row: u16) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

/// Something a mouse click can land on.
///
/// Immediate mode keeps no widget tree, so there is nothing to ask "what is at
/// this cell?". Each frame the renderers record the rectangle of every clickable
/// thing, and clicks are resolved against that list.
/// What clicking a modal control should do.
///
/// Not every control activates: a click that focuses a text field must not also
/// submit the modal, and clicking `◂ value ▸` should advance the value rather
/// than accept the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalClick {
    /// Buttons and checkboxes: focus, then activate.
    Activate,
    /// Text inputs: focus only.
    Focus,
    /// A `◂ value ▸` cycle: focus, then advance one step.
    Cycle,
    /// A row of a list: focus the list and select that row.
    Row(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitTarget {
    SidebarRow(usize),
    /// The `▾`/`▸` twisty on a repository row, which folds it away without
    /// changing the selection.
    SidebarToggle(usize),
    DetailItem(usize),
    /// A key in the footer bar. Clicking one is the same as pressing it, so the
    /// character is carried rather than a resolved action.
    FooterKey(char),
    /// Focus index within the modal on top of the stack, and what a click does.
    ModalControl {
        index: usize,
        click: ModalClick,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct Hit {
    pub rect: ratatui::layout::Rect,
    pub target: HitTarget,
}

impl App {
    // -------------------------------------------------------------------- mouse

    /// Drop the previous frame's clickable regions. Called at the start of draw.
    pub fn clear_hits(&mut self) {
        self.hits.clear();
    }

    pub fn push_hit(&mut self, rect: ratatui::layout::Rect, target: HitTarget) {
        self.hits.push(Hit { rect, target });
    }

    /// Topmost target at a cell. Later hits win, so a modal drawn over the panes
    /// takes the click rather than whatever it covers.
    pub fn hit_at(&self, column: u16, row: u16) -> Option<HitTarget> {
        // `contains` saturates, so a region touching the terminal's far edge
        // cannot overflow `u16` and panic in a debug build.
        self.hits
            .iter()
            .rev()
            .find(|hit| contains(hit.rect, column, row))
            .map(|hit| hit.target)
    }

    pub fn handle_mouse(&mut self, mouse: ratatui::crossterm::event::MouseEvent) {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};

        match mouse.kind {
            MouseEventKind::ScrollDown => {
                self.scroll_at(mouse.column, mouse.row, 1);
                return;
            }
            MouseEventKind::ScrollUp => {
                self.scroll_at(mouse.column, mouse.row, -1);
                return;
            }
            // Hover. The pointer crossing cells inside one control changes
            // nothing on screen, so only a change of target costs a repaint.
            MouseEventKind::Moved => {
                let target = self.hit_at(mouse.column, mouse.row);
                if target != self.hovered {
                    self.hovered = target;
                    self.redraw = true;
                }
                return;
            }
            // Dragging the scrollbar. The pointer routinely leaves the track
            // mid-drag, so this is not gated on where it currently is — only on
            // a drag having started there.
            MouseEventKind::Drag(MouseButton::Left) => {
                self.drag_scrollbar(mouse.row);
                return;
            }
            // The release carries a position too, and it is often past the last
            // drag report — a quick flick ends with the button coming up several
            // rows beyond where the last motion landed. Applying it before
            // letting go means the thumb finishes where the pointer did.
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag_scrollbar(mouse.row);
                self.scroll_drag = None;
                return;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.grab_scrollbar(mouse.column, mouse.row) {
                    return;
                }
            }
            _ => return,
        }

        let Some(target) = self.hit_at(mouse.column, mouse.row) else {
            return;
        };

        // A modal owns every click while it is open, including the ones that
        // land on the panes behind it.
        if !self.modals.is_empty() {
            if let HitTarget::ModalControl { index, click } = target {
                self.click_modal_control(index, click);
            }
            return;
        }

        match target {
            HitTarget::SidebarRow(index) => {
                self.focus = Focus::Sidebar;
                self.sidebar_index = index;
                self.select_current_row();
            }
            HitTarget::SidebarToggle(index) => self.toggle_collapsed(index),
            HitTarget::DetailItem(index) => {
                self.focus = Focus::Detail;
                self.detail_index = index;
                self.detail_follow_focus = true;
                // Resolve against the drawn snapshot, not a re-derived list: a
                // background event drained in this same batch can have grown
                // the live list, remapping this index onto a different control
                // than the one the click landed on. A disabled control keeps
                // its slot but must not fire.
                if let Some((DetailItem::Action(action), enabled)) =
                    self.drawn_items.get(index).cloned()
                    && enabled
                {
                    self.run_action(action);
                }
            }
            HitTarget::FooterKey(key) => {
                self.handle_key(ratatui::crossterm::event::KeyEvent::new(
                    ratatui::crossterm::event::KeyCode::Char(key),
                    ratatui::crossterm::event::KeyModifiers::NONE,
                ));
            }
            HitTarget::ModalControl { .. } => {}
        }
    }

    /// Fold a repository row shut, or open it again.
    ///
    /// Folding is a view operation and leaves the selection alone — unless the
    /// selected worktree is the thing being hidden. Keeping it would leave the
    /// detail pane describing a row that is no longer on screen, with the
    /// sidebar cursor pointing at whatever slid into its place, so the selection
    /// falls back to the repository that swallowed it.
    pub fn toggle_collapsed(&mut self, index: usize) {
        let Some(SidebarRow::Repository { id, .. }) = self.rows.get(index) else {
            return;
        };
        let id = *id;
        let collapsing = self.collapsed.insert(id);
        if !collapsing {
            self.collapsed.remove(&id);
        }

        let hides_selection = collapsing
            && self.state.selection.repository_id == Some(id)
            && self.state.selection.worktree_id.is_some();
        if hides_selection {
            self.state.select_repository(id);
            self.rebuild_rows();
            self.sync_sidebar_index();
            self.reload_detail();
            return;
        }

        self.rebuild_rows();
        self.sync_sidebar_index();
    }

    /// Route the wheel to the pane the pointer is over, not the focused one.
    ///
    /// The pointer is the thing the user is aiming with; Textual scrolled the
    /// widget under it, and scrolling a pane the mouse is nowhere near is the
    /// behaviour this replaces.
    fn scroll_at(&mut self, column: u16, row: u16, delta: isize) {
        if contains(self.sidebar_rect, column, row) {
            self.scroll_sidebar(delta);
        } else if contains(self.detail_rect, column, row) {
            self.scroll_detail(delta);
        }
    }

    /// Start a scrollbar drag, if the press landed on the bar.
    ///
    /// Pressing the thumb picks it up where it was grabbed. Pressing the bare
    /// track pages the thumb to the pointer first and then behaves as if it had
    /// been grabbed by its middle, so the press and any drag that follows are
    /// one continuous gesture rather than a jump and then a second one.
    ///
    /// Focus is left alone on purpose: scrolling something is not the same as
    /// aiming at it, and stealing focus here would move the keyboard cursor into
    /// the pane every time the user only wanted to read further down.
    fn grab_scrollbar(&mut self, column: u16, row: u16) -> bool {
        let Some(geom) = self.scrollbar else {
            return false;
        };
        if !contains(geom.track, column, row) {
            return false;
        }

        let local = row - geom.track.y;
        let top = geom.thumb_top(self.detail_scroll);
        let grab = if local >= top && local < top.saturating_add(geom.thumb) {
            local - top
        } else {
            let middle = geom.thumb / 2;
            self.detail_scroll = geom.offset_at(local.saturating_sub(middle));
            middle.min(local)
        };

        self.scroll_drag = Some(grab);
        true
    }

    /// Continue a scrollbar drag. Silent unless a drag is actually in progress,
    /// so a drag begun anywhere else cannot move the pane.
    fn drag_scrollbar(&mut self, row: u16) {
        let (Some(geom), Some(grab)) = (self.scrollbar, self.scroll_drag) else {
            return;
        };
        // Above the track this saturates to the top; below it, clamping to the
        // last row and letting `offset_at` cap the result pins it to the bottom.
        let local = row
            .saturating_sub(geom.track.y)
            .min(geom.track.height.saturating_sub(1));
        let next = geom.offset_at(local.saturating_sub(grab));
        if next != self.detail_scroll {
            self.detail_scroll = next;
            self.redraw = true;
        }
    }

    /// Scroll the detail pane's viewport. Deliberately independent of the focus
    /// ring: reading something is not the same as aiming at it.
    pub(super) fn scroll_detail(&mut self, delta: isize) {
        let next = (self.detail_scroll as isize + delta * SCROLL_STEP).max(0);
        // The upper bound depends on content height, which only the renderer
        // knows; it clamps there and writes the value back.
        self.detail_scroll = next as u16;
    }

    fn scroll_sidebar(&mut self, delta: isize) {
        self.move_sidebar(delta);
    }

    /// Apply a click to a modal control: always focus it, then do whatever that
    /// kind of control does when driven from the keyboard.
    fn click_modal_control(&mut self, index: usize, click: ModalClick) {
        use ratatui::crossterm::event::KeyCode;

        if let Some(modal) = self.modals.last_mut() {
            modal.set_focus(index);
            if let ModalClick::Row(row) = click {
                modal.set_row(row);
            }
        }
        match click {
            ModalClick::Activate => self.send_modal_key(KeyCode::Enter),
            ModalClick::Cycle => self.send_modal_key(KeyCode::Right),
            // Focusing a field must not submit the dialog, and selecting a row
            // is already done above.
            ModalClick::Focus | ModalClick::Row(_) => {}
        }
    }

    fn send_modal_key(&mut self, code: ratatui::crossterm::event::KeyCode) {
        use ratatui::crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        self.handle_modal_key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
    }
}
