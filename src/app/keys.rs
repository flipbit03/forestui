//! Keyboard input: global hotkeys, focus movement, and the rename fields.

use super::{Action, App, DetailItem, Field, Focus, PAGE_STEP};
use crate::event::Severity;
use crate::modal::{AddRepositoryModal, Modal, SettingsModal};
use crate::ui::widgets::TextInput;

/// Every footer binding, in the order Textual's `Footer` rendered them: `q`
/// is declared first in the Python bindings but carries `priority=True`,
/// which sorted it after `a`. One table drives both the footer and the `?`
/// help toast, so the two lists cannot drift; `handle_key` below must handle
/// every character listed here.
pub const BINDINGS: [(char, &str); 12] = [
    ('a', "Add Repo"),
    ('q', "Quit"),
    ('w', "Add Worktree"),
    ('e', "Editor"),
    ('t', "Terminal"),
    ('o', "Files"),
    ('n', "Claude"),
    ('y', "ClaudeYOLO"),
    ('h', "Archive"),
    ('d', "Delete"),
    ('s', "Settings"),
    ('?', "Help"),
];

/// Bindings that work everywhere but never fit Textual's footer. The help
/// toast lists them; the footer does not, for parity with the Python build.
pub const EXTRA_BINDINGS: [(char, &str); 2] = [('r', "Refresh"), ('A', "Show Archived")];

impl App {
    // --------------------------------------------------------------------- keys

    pub(super) fn handle_key(&mut self, key: ratatui::crossterm::event::KeyEvent) {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        if !self.modals.is_empty() {
            self.handle_modal_key(key);
            return;
        }

        // A focused text field owns every printable key. Resolved against the
        // drawn snapshot rather than re-walking the pane content on every
        // keypress — the snapshot is also what Enter and the mouse act on.
        if self.focus == Focus::Detail
            && let Some((DetailItem::Field(field), _)) =
                self.drawn_items.get(self.detail_index).cloned()
            && self.handle_field_key(field, key)
        {
            return;
        }

        match key.code {
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Detail,
                    Focus::Detail => Focus::Sidebar,
                };
                // Entering the pane should show where the cursor landed.
                self.detail_follow_focus = true;
                return;
            }
            KeyCode::Up => {
                match self.focus {
                    Focus::Sidebar => self.move_sidebar(-1),
                    Focus::Detail => {
                        self.detail_index = self.detail_index.saturating_sub(1);
                        self.detail_follow_focus = true;
                    }
                }
                return;
            }
            KeyCode::Down => {
                match self.focus {
                    Focus::Sidebar => self.move_sidebar(1),
                    Focus::Detail => {
                        let len = self.drawn_items.len();
                        if len > 0 {
                            self.detail_index = (self.detail_index + 1).min(len - 1);
                        }
                        self.detail_follow_focus = true;
                    }
                }
                return;
            }
            // Textual bound these to the viewport rather than the focus ring, so
            // in the detail pane they page without moving the cursor.
            KeyCode::PageUp => {
                match self.focus {
                    Focus::Sidebar => self.move_sidebar(-PAGE_STEP),
                    Focus::Detail => self.scroll_detail(-PAGE_STEP),
                }
                return;
            }
            KeyCode::PageDown => {
                match self.focus {
                    Focus::Sidebar => self.move_sidebar(PAGE_STEP),
                    Focus::Detail => self.scroll_detail(PAGE_STEP),
                }
                return;
            }
            KeyCode::Enter => {
                match self.focus {
                    Focus::Sidebar => self.select_current_row(),
                    Focus::Detail => {
                        // The drawn snapshot, for the same reason as the mouse
                        // path: act on what the user saw, and never fire a
                        // control the frame showed as disabled.
                        if let Some((DetailItem::Action(action), enabled)) =
                            self.drawn_items.get(self.detail_index).cloned()
                            && enabled
                        {
                            self.run_action(action);
                        }
                    }
                }
                return;
            }
            _ => {}
        }

        if let KeyCode::Char(binding) = key.code {
            self.run_binding(binding);
        }
    }

    /// Run a footer binding by its character.
    ///
    /// Split out because clicking the footer is not the same as typing: the
    /// click already says which binding was meant, so it must not be re-routed
    /// into a focused rename field, where `d Delete` would have been typed into
    /// the name instead of deleting anything.
    pub(super) fn run_binding(&mut self, binding: char) {
        use ratatui::crossterm::event::KeyCode;

        match KeyCode::Char(binding) {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('a') => self
                .modals
                .push(Modal::AddRepository(AddRepositoryModal::new())),
            KeyCode::Char('w') => self.action_add_worktree(),
            KeyCode::Char('e') => self.run_action(Action::Editor),
            KeyCode::Char('t') => self.run_action(Action::Terminal),
            KeyCode::Char('o') => self.run_action(Action::Files),
            KeyCode::Char('n') => self.run_action(Action::ClaudeNew),
            KeyCode::Char('y') => self.run_action(Action::ClaudeYolo),
            KeyCode::Char('h') => self.action_toggle_archive(),
            KeyCode::Char('d') => self.action_delete(),
            KeyCode::Char('s') => self
                .modals
                .push(Modal::Settings(Box::new(SettingsModal::new(
                    &self.settings,
                )))),
            KeyCode::Char('r') => {
                self.rebuild_rows();
                self.reload_detail();
            }
            KeyCode::Char('A') => {
                self.state.show_archived = !self.state.show_archived;
                self.rebuild_rows();
            }
            KeyCode::Char('?') => {
                // Derived from the binding tables, so a key can no longer be
                // reachable yet undiscoverable — 'o', 'y', 'r' and 'A' were.
                let help = BINDINGS
                    .iter()
                    .chain(EXTRA_BINDINGS.iter())
                    .map(|(key, label)| format!("{key}: {label}"))
                    .collect::<Vec<_>>()
                    .join(" | ");
                self.notify(help, Severity::Information);
            }
            _ => {}
        }
    }

    /// Route a key to a focused rename field. Returns true when consumed.
    fn handle_field_key(&mut self, field: Field, key: ratatui::crossterm::event::KeyEvent) -> bool {
        use ratatui::crossterm::event::KeyCode;

        if matches!(
            key.code,
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down
        ) {
            return false;
        }
        if key.code == KeyCode::Enter {
            self.submit_rename(field);
            return true;
        }

        // Escape has to hand focus back as well as undo the edit — a focused
        // field swallows every printable key, so the global hotkeys are
        // unreachable from here otherwise.
        if key.code == KeyCode::Esc {
            self.reset_rename_fields();
            self.focus = Focus::Sidebar;
            return true;
        }

        let input = match field {
            Field::WorktreeName => &mut self.name_input,
            Field::BranchName => &mut self.branch_input,
        };
        input.apply_edit_key(key)
    }

    fn reset_rename_fields(&mut self) {
        if let Some((_repo, worktree)) = self.state.selected_worktree() {
            self.name_input = TextInput::new(worktree.name.clone());
            self.branch_input = TextInput::new(worktree.branch.clone());
        }
    }
}
