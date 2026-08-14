//! Modal dialogs: their state, focus model, and key handling.
//!
//! Textual pushed modal *screens* and awaited their result. Here modals live on
//! an explicit stack (`Vec<Modal>`), because the settings flow genuinely nests
//! three deep: Settings → Custom Buttons → Edit Button. A child that closes with
//! a value hands it to its parent through [`Modal::receive_child`].

use crate::models::{
    CustomClaudeButton, GitHubIssue, MAX_BUTTON_LABEL_LENGTH, MAX_BUTTON_PREFIX_LENGTH,
    MAX_CLAUDE_COMMAND_LENGTH, Repository, Settings, derive_prefix, validate_button_label,
    validate_button_prefix, validate_claude_command,
};
use crate::ui::widgets::TextInput;
use crate::util;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;
use uuid::Uuid;

pub const EDITORS: [(&str, &str); 10] = [
    ("VS Code", "code"),
    ("Cursor", "cursor"),
    ("Neovim (tmux)", "nvim"),
    ("Vim (tmux)", "vim"),
    ("Helix (tmux)", "hx"),
    ("Emacs TUI (tmux)", "emacs -nw"),
    ("PyCharm", "pycharm"),
    ("Sublime Text", "subl"),
    ("Nano (tmux)", "nano"),
    ("Micro (tmux)", "micro"),
];

pub const THEMES: [(&str, &str); 3] = [("System", "system"), ("Dark", "dark"), ("Light", "light")];

/// What the app must do after a modal handled a key.
#[derive(Debug)]
pub enum ModalOutcome {
    /// Nothing to do; the modal keeps the focus.
    None,
    /// Dismiss the top modal without a result.
    Close,
    /// Open a nested modal.
    Push(Box<Modal>),
    /// Dismiss the top modal and act on its result.
    Submit(ModalResult),
    /// Run a side effect; the modal stays open.
    Effect(ModalEffect),
}

#[derive(Debug, Clone)]
pub enum ModalEffect {
    /// `git fetch` in the given repository, then refresh the branch list.
    Fetch(String),
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    DeleteWorktree(Uuid),
    RemoveRepository(Uuid),
}

#[derive(Debug, Clone)]
pub enum ModalResult {
    RepositoryAdded {
        path: String,
        import_worktrees: bool,
    },
    WorktreeCreated {
        repo_id: Uuid,
        name: String,
        branch: String,
        new_branch: bool,
    },
    WorktreeFromIssue {
        repo_id: Uuid,
        name: String,
        branch: String,
        pull_first: bool,
        base_branch: Option<String>,
    },
    SettingsSaved(Box<Settings>),
    CustomButtonsSaved(Vec<CustomClaudeButton>),
    CustomButtonSaved(Box<CustomClaudeButton>),
    Confirmed(ConfirmAction),
}

#[derive(Debug)]
pub enum Modal {
    AddRepository(AddRepositoryModal),
    AddWorktree(Box<AddWorktreeModal>),
    CreateFromIssue(Box<CreateFromIssueModal>),
    Settings(Box<SettingsModal>),
    CustomButtons(CustomButtonsModal),
    EditButton(Box<EditButtonModal>),
    Confirm(ConfirmModal),
}

impl Modal {
    pub fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        match self {
            Modal::AddRepository(m) => m.handle_key(key),
            Modal::AddWorktree(m) => m.handle_key(key),
            Modal::CreateFromIssue(m) => m.handle_key(key),
            Modal::Settings(m) => m.handle_key(key),
            Modal::CustomButtons(m) => m.handle_key(key),
            Modal::EditButton(m) => m.handle_key(key),
            Modal::Confirm(m) => m.handle_key(key),
        }
    }

    /// Deliver a nested modal's result to its parent.
    pub fn receive_child(&mut self, result: &ModalResult) {
        match (self, result) {
            (Modal::Settings(parent), ModalResult::CustomButtonsSaved(buttons)) => {
                parent.custom_buttons = buttons.clone();
            }
            (Modal::CustomButtons(parent), ModalResult::CustomButtonSaved(button)) => {
                parent.apply_edit((**button).clone());
            }
            _ => {}
        }
    }

    /// Advance any spinner this modal owns.
    pub fn tick(&mut self) {
        if let Modal::CreateFromIssue(m) = self
            && m.is_fetching
        {
            m.spinner_index = (m.spinner_index + 1) % SPINNER.len();
        }
    }
}

pub const SPINNER: [char; 4] = ['|', '/', '-', '\\'];

fn is_escape(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc
}

/// Cycle a focus index with Tab / Shift+Tab / Up / Down.
fn cycle_focus(focus: &mut usize, len: usize, key: KeyEvent) -> bool {
    if len == 0 {
        return false;
    }
    match key.code {
        KeyCode::Tab | KeyCode::Down => {
            *focus = (*focus + 1) % len;
            true
        }
        KeyCode::BackTab | KeyCode::Up => {
            *focus = if *focus == 0 { len - 1 } else { *focus - 1 };
            true
        }
        _ => false,
    }
}

/// Apply an editing key to a text input. Returns true when the key was consumed.
fn edit_input(input: &mut TextInput, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            input.kill_to_start();
            true
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            input.insert(c);
            true
        }
        KeyCode::Backspace => {
            input.backspace();
            true
        }
        KeyCode::Delete => {
            input.delete();
            true
        }
        KeyCode::Left => {
            input.move_left();
            true
        }
        KeyCode::Right => {
            input.move_right();
            true
        }
        KeyCode::Home => {
            input.move_home();
            true
        }
        KeyCode::End => {
            input.move_end();
            true
        }
        _ => false,
    }
}

// ---------------------------------------------------------------- Add repository

#[derive(Debug)]
pub struct AddRepositoryModal {
    pub path: TextInput,
    pub import_worktrees: bool,
    /// Focus: 0 path, 1 import checkbox, 2 Add, 3 Cancel.
    pub focus: usize,
}

impl Default for AddRepositoryModal {
    fn default() -> Self {
        Self::new()
    }
}

impl AddRepositoryModal {
    pub const FIELDS: usize = 4;

    pub fn new() -> Self {
        Self {
            path: TextInput::new("").with_placeholder("Enter path or paste from clipboard..."),
            import_worktrees: false,
            focus: 0,
        }
    }

    /// Validation message shown under the input, and whether the path is usable.
    pub fn status(&self) -> (String, bool) {
        let raw = self.path.value();
        if raw.is_empty() {
            return (String::new(), false);
        }
        let path = util::expanduser(raw);
        if !path.exists() {
            return ("Path does not exist".into(), false);
        }
        if !path.is_dir() {
            return ("Path is not a directory".into(), false);
        }
        if !path.join(".git").exists() {
            return ("Not a git repository".into(), false);
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        (format!("Repository: {name}"), true)
    }

    fn submit(&self) -> ModalOutcome {
        let (_, valid) = self.status();
        if !valid {
            return ModalOutcome::None;
        }
        let path = util::expanduser(self.path.value())
            .to_string_lossy()
            .to_string();
        ModalOutcome::Submit(ModalResult::RepositoryAdded {
            path,
            import_worktrees: self.import_worktrees,
        })
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return ModalOutcome::Close;
        }
        // While editing the path, arrows move the cursor rather than the focus.
        if self.focus == 0 && edit_input(&mut self.path, key) {
            return ModalOutcome::None;
        }
        if cycle_focus(&mut self.focus, Self::FIELDS, key) {
            return ModalOutcome::None;
        }
        match (self.focus, key.code) {
            (1, KeyCode::Char(' ')) | (1, KeyCode::Enter) => {
                self.import_worktrees = !self.import_worktrees;
                ModalOutcome::None
            }
            (0 | 2, KeyCode::Enter) => self.submit(),
            (3, KeyCode::Enter) => ModalOutcome::Close,
            _ => ModalOutcome::None,
        }
    }
}

// ------------------------------------------------------------------ Add worktree

#[derive(Debug)]
pub struct AddWorktreeModal {
    pub repo_id: Uuid,
    pub repo_name: String,
    pub branches: Vec<String>,
    pub remotes: Vec<String>,
    pub forest_dir: PathBuf,
    pub branch_prefix: String,
    pub name: TextInput,
    pub branch: TextInput,
    pub search: TextInput,
    pub new_branch: bool,
    pub search_index: usize,
    pub error: String,
    /// Focus: 0 name, 1 mode, 2 branch/search, 3 results (search mode), then Create, Cancel.
    pub focus: usize,
}

impl AddWorktreeModal {
    pub fn new(
        repo: &Repository,
        branches: Vec<String>,
        remotes: Vec<String>,
        forest_dir: PathBuf,
        branch_prefix: String,
    ) -> Self {
        Self {
            repo_id: repo.id,
            repo_name: repo.name.clone(),
            branches,
            remotes,
            forest_dir,
            branch_prefix: branch_prefix.clone(),
            name: TextInput::new("").with_placeholder("my-feature"),
            branch: TextInput::new("").with_placeholder(format!("{branch_prefix}my-feature")),
            search: TextInput::new("").with_placeholder("Start typing to search branches..."),
            new_branch: true,
            search_index: 0,
            error: String::new(),
            focus: 0,
        }
    }

    pub fn field_count(&self) -> usize {
        if self.new_branch { 5 } else { 6 }
    }

    fn create_index(&self) -> usize {
        self.field_count() - 2
    }

    fn cancel_index(&self) -> usize {
        self.field_count() - 1
    }

    /// Index of the results list, which only exists in existing-branch mode.
    fn results_index(&self) -> Option<usize> {
        if self.new_branch { None } else { Some(3) }
    }

    pub fn matches(&self) -> Vec<(String, f64)> {
        util::fuzzy_match_branches(
            self.search.value(),
            &self.branches,
            &self.remotes,
            util::MAX_DROPDOWN_RESULTS,
        )
    }

    /// Sanitised worktree name: alphanumerics, `-` and `_` only.
    pub fn sanitized_name(&self) -> String {
        self.name
            .value()
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .collect()
    }

    pub fn path_preview(&self) -> Option<PathBuf> {
        let name = self.sanitized_name();
        if name.is_empty() {
            return None;
        }
        Some(self.forest_dir.join(&self.repo_name).join(name))
    }

    pub fn selected_branch(&self) -> String {
        if self.new_branch {
            self.branch.value().to_string()
        } else {
            self.search.value().to_string()
        }
    }

    /// The Create control is disabled when an existing branch is not real.
    pub fn can_create(&self) -> bool {
        if self.new_branch {
            return true;
        }
        let branch = self.selected_branch();
        self.branches.contains(&branch)
    }

    fn set_mode(&mut self, new_branch: bool) {
        self.new_branch = new_branch;
        self.focus = self.focus.min(self.field_count() - 1);
        self.error.clear();
    }

    fn submit(&mut self) -> ModalOutcome {
        let name = self.sanitized_name();
        if name.is_empty() {
            self.error = "Worktree name is required".into();
            return ModalOutcome::None;
        }
        let branch = self.selected_branch();
        if branch.is_empty() {
            self.error = "Branch name is required".into();
            return ModalOutcome::None;
        }
        if !self.new_branch && !self.branches.contains(&branch) {
            self.error = format!("Branch '{branch}' does not exist");
            return ModalOutcome::None;
        }
        if let Some(path) = self.path_preview()
            && path.exists()
        {
            self.error = "Worktree path already exists".into();
            return ModalOutcome::None;
        }
        ModalOutcome::Submit(ModalResult::WorktreeCreated {
            repo_id: self.repo_id,
            name,
            branch,
            new_branch: self.new_branch,
        })
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return ModalOutcome::Close;
        }

        // Text editing takes precedence while a field has focus.
        if self.focus == 0 && edit_input(&mut self.name, key) {
            self.error.clear();
            // A new branch name tracks the worktree name.
            if self.new_branch {
                self.branch
                    .set_value(format!("{}{}", self.branch_prefix, self.sanitized_name()));
            }
            return ModalOutcome::None;
        }
        if self.focus == 2 {
            let input = if self.new_branch {
                &mut self.branch
            } else {
                &mut self.search
            };
            if edit_input(input, key) {
                self.error.clear();
                self.search_index = 0;
                return ModalOutcome::None;
            }
        }

        // The results list owns Up/Down when it has focus.
        if self.results_index() == Some(self.focus) {
            let count = self.matches().len();
            match key.code {
                KeyCode::Up => {
                    self.search_index = self.search_index.saturating_sub(1);
                    return ModalOutcome::None;
                }
                KeyCode::Down => {
                    if count > 0 {
                        self.search_index = (self.search_index + 1).min(count - 1);
                    }
                    return ModalOutcome::None;
                }
                KeyCode::Enter => {
                    if let Some((branch, _)) = self.matches().into_iter().nth(self.search_index) {
                        self.search.set_value(branch);
                    }
                    return ModalOutcome::None;
                }
                _ => {}
            }
        }

        if self.focus == 1 {
            match key.code {
                KeyCode::Left => {
                    self.set_mode(true);
                    return ModalOutcome::None;
                }
                KeyCode::Right => {
                    self.set_mode(false);
                    return ModalOutcome::None;
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let next = !self.new_branch;
                    self.set_mode(next);
                    return ModalOutcome::None;
                }
                _ => {}
            }
        }

        let field_count = self.field_count();
        if cycle_focus(&mut self.focus, field_count, key) {
            return ModalOutcome::None;
        }

        if key.code == KeyCode::Enter {
            if self.focus == self.cancel_index() {
                return ModalOutcome::Close;
            }
            if self.focus == self.create_index() || self.focus == 0 || self.focus == 2 {
                if !self.can_create() {
                    self.error = format!("Branch '{}' does not exist", self.selected_branch());
                    return ModalOutcome::None;
                }
                return self.submit();
            }
        }
        ModalOutcome::None
    }
}

// -------------------------------------------------------- Create worktree from issue

#[derive(Debug)]
pub struct CreateFromIssueModal {
    pub repo_id: Uuid,
    pub repo_name: String,
    pub repo_path: String,
    pub issue_number: i64,
    pub issue_title: String,
    pub branches: Vec<String>,
    pub remotes: Vec<String>,
    pub forest_dir: PathBuf,
    pub name: TextInput,
    pub branch: TextInput,
    pub base_branch: TextInput,
    pub pull_first: bool,
    pub is_fetching: bool,
    pub spinner_index: usize,
    /// Focus: 0 name, 1 branch, 2 base branch, 3 Fetch, 4 pull checkbox, 5 Create, 6 Cancel.
    pub focus: usize,
}

impl CreateFromIssueModal {
    pub const FIELDS: usize = 7;

    pub fn new(
        repo: &Repository,
        issue: &GitHubIssue,
        branches: Vec<String>,
        remotes: Vec<String>,
        forest_dir: PathBuf,
        branch_prefix: &str,
        current_branch: &str,
    ) -> Self {
        let issue_branch = issue.branch_name();
        let base = default_base_branch(&branches, &remotes, current_branch);
        Self {
            repo_id: repo.id,
            repo_name: repo.name.clone(),
            repo_path: repo.source_path.clone(),
            issue_number: issue.number,
            issue_title: issue.title.clone(),
            branches,
            remotes,
            forest_dir,
            name: TextInput::new(issue_branch.clone()).with_placeholder("worktree-name"),
            branch: TextInput::new(format!("{branch_prefix}{issue_branch}"))
                .with_placeholder("feat/branch-name"),
            base_branch: TextInput::new(base).with_placeholder("origin/main"),
            pull_first: true,
            is_fetching: false,
            spinner_index: 0,
            focus: 0,
        }
    }

    pub fn path_preview(&self) -> PathBuf {
        self.forest_dir
            .join(&self.repo_name)
            .join(self.name.value())
    }

    /// The inline suggestion shown after the typed base branch.
    pub fn base_suggestion(&self) -> Option<String> {
        let value = self.base_branch.value();
        if value.is_empty() {
            return None;
        }
        util::fuzzy_match_branches(value, &self.branches, &self.remotes, 1)
            .into_iter()
            .next()
            .map(|(b, _)| b)
    }

    pub fn can_create(&self) -> bool {
        let base = self.base_branch.value();
        base.is_empty() || self.branches.iter().any(|b| b == base)
    }

    /// Apply a refreshed branch list after a fetch.
    pub fn update_branches(&mut self, branches: Vec<String>, remotes: Vec<String>, current: &str) {
        self.is_fetching = false;
        self.branches = branches;
        self.remotes = remotes;
        let base = self.base_branch.value().to_string();
        if base.is_empty() || !self.branches.contains(&base) {
            let next = default_base_branch(&self.branches, &self.remotes, current);
            self.base_branch.set_value(next);
        }
    }

    pub fn fetch_failed(&mut self) {
        self.is_fetching = false;
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return ModalOutcome::Close;
        }

        let editable = match self.focus {
            0 => Some(&mut self.name),
            1 => Some(&mut self.branch),
            2 => Some(&mut self.base_branch),
            _ => None,
        };
        if let Some(input) = editable
            && edit_input(input, key)
        {
            return ModalOutcome::None;
        }

        if self.focus == 2
            && key.code == KeyCode::Right
            && let Some(suggestion) = self.base_suggestion()
        {
            self.base_branch.set_value(suggestion);
            return ModalOutcome::None;
        }

        if cycle_focus(&mut self.focus, Self::FIELDS, key) {
            return ModalOutcome::None;
        }

        match (self.focus, key.code) {
            (3, KeyCode::Enter) => {
                if self.is_fetching {
                    return ModalOutcome::None;
                }
                self.is_fetching = true;
                self.spinner_index = 0;
                ModalOutcome::Effect(ModalEffect::Fetch(self.repo_path.clone()))
            }
            (4, KeyCode::Enter) | (4, KeyCode::Char(' ')) => {
                self.pull_first = !self.pull_first;
                ModalOutcome::None
            }
            (6, KeyCode::Enter) => ModalOutcome::Close,
            (_, KeyCode::Enter) => {
                if self.name.is_empty() || self.branch.is_empty() || !self.can_create() {
                    return ModalOutcome::None;
                }
                let base = self.base_branch.value().to_string();
                ModalOutcome::Submit(ModalResult::WorktreeFromIssue {
                    repo_id: self.repo_id,
                    name: self.name.value().to_string(),
                    branch: self.branch.value().to_string(),
                    pull_first: self.pull_first,
                    base_branch: if base.is_empty() { None } else { Some(base) },
                })
            }
            _ => ModalOutcome::None,
        }
    }
}

/// Prefer `<remote>/<current>` when it exists, then the local branch, then the
/// first branch in the list.
pub fn default_base_branch(branches: &[String], remotes: &[String], current: &str) -> String {
    for remote in remotes {
        let candidate = format!("{remote}/{current}");
        if branches.contains(&candidate) {
            return candidate;
        }
    }
    if branches.iter().any(|b| b == current) {
        return current.to_string();
    }
    branches.first().cloned().unwrap_or_default()
}

// ---------------------------------------------------------------------- Settings

#[derive(Debug)]
pub struct SettingsModal {
    pub editor_index: usize,
    pub theme_index: usize,
    pub branch_prefix: TextInput,
    pub custom_buttons: Vec<CustomClaudeButton>,
    /// Focus: 0 editor, 1 branch prefix, 2 theme, 3 manage buttons, 4 Save, 5 Cancel.
    pub focus: usize,
}

impl SettingsModal {
    pub const FIELDS: usize = 6;

    pub fn new(settings: &Settings) -> Self {
        Self {
            editor_index: EDITORS
                .iter()
                .position(|(_, cmd)| *cmd == settings.default_editor)
                .unwrap_or(0),
            theme_index: THEMES
                .iter()
                .position(|(_, value)| *value == settings.theme)
                .unwrap_or(0),
            branch_prefix: TextInput::new(settings.branch_prefix.clone()).with_placeholder("feat/"),
            custom_buttons: settings.custom_buttons.clone(),
            focus: 0,
        }
    }

    pub fn buttons_summary(&self) -> String {
        match self.custom_buttons.len() {
            0 => "No custom buttons configured".into(),
            1 => "1 custom button configured".into(),
            n => format!("{n} custom buttons configured"),
        }
    }

    fn to_settings(&self) -> Settings {
        Settings {
            default_editor: EDITORS[self.editor_index].1.to_string(),
            default_terminal: String::new(),
            branch_prefix: self.branch_prefix.value().to_string(),
            theme: THEMES[self.theme_index].1.to_string(),
            custom_buttons: self.custom_buttons.clone(),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return ModalOutcome::Close;
        }
        if self.focus == 1 && edit_input(&mut self.branch_prefix, key) {
            return ModalOutcome::None;
        }

        match (self.focus, key.code) {
            (0, KeyCode::Left) => {
                self.editor_index = wrap_dec(self.editor_index, EDITORS.len());
                return ModalOutcome::None;
            }
            (0, KeyCode::Right) => {
                self.editor_index = (self.editor_index + 1) % EDITORS.len();
                return ModalOutcome::None;
            }
            (2, KeyCode::Left) => {
                self.theme_index = wrap_dec(self.theme_index, THEMES.len());
                return ModalOutcome::None;
            }
            (2, KeyCode::Right) => {
                self.theme_index = (self.theme_index + 1) % THEMES.len();
                return ModalOutcome::None;
            }
            _ => {}
        }

        if cycle_focus(&mut self.focus, Self::FIELDS, key) {
            return ModalOutcome::None;
        }

        match (self.focus, key.code) {
            (3, KeyCode::Enter) => ModalOutcome::Push(Box::new(Modal::CustomButtons(
                CustomButtonsModal::new(self.custom_buttons.clone()),
            ))),
            (5, KeyCode::Enter) => ModalOutcome::Close,
            (_, KeyCode::Enter) => {
                ModalOutcome::Submit(ModalResult::SettingsSaved(Box::new(self.to_settings())))
            }
            _ => ModalOutcome::None,
        }
    }
}

fn wrap_dec(index: usize, len: usize) -> usize {
    if index == 0 { len - 1 } else { index - 1 }
}

// ---------------------------------------------------------------- Custom buttons

#[derive(Debug)]
pub struct CustomButtonsModal {
    pub buttons: Vec<CustomClaudeButton>,
    pub selected: usize,
    /// Index being edited, so the child's result lands in the right slot.
    pub editing: Option<usize>,
}

impl CustomButtonsModal {
    pub fn new(buttons: Vec<CustomClaudeButton>) -> Self {
        Self {
            buttons,
            selected: 0,
            editing: None,
        }
    }

    fn apply_edit(&mut self, button: CustomClaudeButton) {
        match self.editing.take() {
            Some(index) if index < self.buttons.len() => self.buttons[index] = button,
            _ => self.buttons.push(button),
        }
    }

    fn swap(&mut self, index: usize, delta: isize) {
        let target = index as isize + delta;
        if target < 0 || target as usize >= self.buttons.len() {
            return;
        }
        let target = target as usize;
        self.buttons.swap(index, target);
        self.selected = target;
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return ModalOutcome::Close;
        }
        let len = self.buttons.len();
        match key.code {
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                if len > 0 {
                    self.selected = (self.selected + 1).min(len - 1);
                }
            }
            KeyCode::Char('a') => {
                self.editing = None;
                return ModalOutcome::Push(Box::new(Modal::EditButton(Box::new(
                    EditButtonModal::new(None, &self.buttons, None),
                ))));
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                if self.selected < len {
                    self.editing = Some(self.selected);
                    return ModalOutcome::Push(Box::new(Modal::EditButton(Box::new(
                        EditButtonModal::new(
                            Some(self.buttons[self.selected].clone()),
                            &self.buttons,
                            Some(self.selected),
                        ),
                    ))));
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if self.selected < len {
                    self.buttons.remove(self.selected);
                    self.selected = self.selected.min(self.buttons.len().saturating_sub(1));
                }
            }
            KeyCode::Char('K') => self.swap(self.selected, -1),
            KeyCode::Char('J') => self.swap(self.selected, 1),
            KeyCode::Char('s') => {
                return ModalOutcome::Submit(ModalResult::CustomButtonsSaved(self.buttons.clone()));
            }
            _ => {}
        }
        ModalOutcome::None
    }
}

// ------------------------------------------------------------------- Edit button

#[derive(Debug)]
pub struct EditButtonModal {
    pub label: TextInput,
    pub prefix: TextInput,
    pub command: TextInput,
    pub is_edit: bool,
    pub other_labels: Vec<String>,
    pub other_prefixes: Vec<String>,
    /// Whether the prefix still auto-follows the label.
    pub follows: bool,
    pub error: String,
    /// Focus: 0 label, 1 prefix, 2 command, 3 Save, 4 Cancel.
    pub focus: usize,
}

impl EditButtonModal {
    pub const FIELDS: usize = 5;

    pub fn new(
        existing: Option<CustomClaudeButton>,
        all: &[CustomClaudeButton],
        editing_index: Option<usize>,
    ) -> Self {
        let others: Vec<&CustomClaudeButton> = all
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != editing_index)
            .map(|(_, b)| b)
            .collect();

        let follows = existing
            .as_ref()
            .map(|b| b.prefix == derive_prefix(&b.label))
            .unwrap_or(true);

        Self {
            label: TextInput::new(
                existing
                    .as_ref()
                    .map(|b| b.label.clone())
                    .unwrap_or_default(),
            )
            .with_placeholder("e.g., YoloDisc")
            .with_max_length(MAX_BUTTON_LABEL_LENGTH),
            prefix: TextInput::new(
                existing
                    .as_ref()
                    .map(|b| b.prefix.clone())
                    .unwrap_or_default(),
            )
            .with_placeholder("e.g., yolodisc")
            .with_max_length(MAX_BUTTON_PREFIX_LENGTH),
            command: TextInput::new(
                existing
                    .as_ref()
                    .map(|b| b.command.clone())
                    .unwrap_or_default(),
            )
            .with_placeholder("e.g., claude --dangerously-skip-permissions")
            .with_max_length(MAX_CLAUDE_COMMAND_LENGTH),
            is_edit: existing.is_some(),
            other_labels: others.iter().map(|b| b.label.clone()).collect(),
            other_prefixes: others.iter().map(|b| b.prefix.clone()).collect(),
            follows,
            error: String::new(),
            focus: 0,
        }
    }

    pub fn title(&self) -> &'static str {
        if self.is_edit {
            "Edit Button"
        } else {
            "Add Button"
        }
    }

    fn save(&mut self) -> ModalOutcome {
        let label = self.label.value().trim().to_string();
        let prefix = self.prefix.value().trim().to_string();
        let command = self.command.value().trim().to_string();

        let command_error = if command.is_empty() {
            Some("Command cannot be empty".to_string())
        } else {
            validate_claude_command(&command)
        };

        // Report the first failing rule, in the order the fields are shown.
        let first_error = [
            validate_button_label(&label),
            validate_button_prefix(&prefix),
            command_error,
        ]
        .into_iter()
        .flatten()
        .next();
        if let Some(error) = first_error {
            self.error = error;
            return ModalOutcome::None;
        }

        if self.other_labels.contains(&label) {
            self.error = "Another button already uses this label".into();
            return ModalOutcome::None;
        }
        if self.other_prefixes.contains(&prefix) {
            self.error = "Another button already uses this prefix".into();
            return ModalOutcome::None;
        }

        self.error.clear();
        ModalOutcome::Submit(ModalResult::CustomButtonSaved(Box::new(
            CustomClaudeButton {
                label,
                prefix,
                command,
            },
        )))
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return ModalOutcome::Close;
        }

        match self.focus {
            0 => {
                if edit_input(&mut self.label, key) {
                    if self.follows {
                        self.prefix.set_value(derive_prefix(self.label.value()));
                    }
                    return ModalOutcome::None;
                }
            }
            1 => {
                if edit_input(&mut self.prefix, key) {
                    self.follows = self.prefix.value() == derive_prefix(self.label.value());
                    return ModalOutcome::None;
                }
            }
            2 if edit_input(&mut self.command, key) => return ModalOutcome::None,
            _ => {}
        }

        if cycle_focus(&mut self.focus, Self::FIELDS, key) {
            return ModalOutcome::None;
        }

        match (self.focus, key.code) {
            (4, KeyCode::Enter) => ModalOutcome::Close,
            (_, KeyCode::Enter) => self.save(),
            _ => ModalOutcome::None,
        }
    }
}

// ----------------------------------------------------------------------- Confirm

#[derive(Debug)]
pub struct ConfirmModal {
    pub title: String,
    pub message: String,
    pub action: ConfirmAction,
    /// `true` when the destructive choice is highlighted.
    pub confirm_focused: bool,
}

impl ConfirmModal {
    pub fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        action: ConfirmAction,
    ) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            action,
            confirm_focused: false,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return ModalOutcome::Close;
        }
        match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                self.confirm_focused = !self.confirm_focused;
                ModalOutcome::None
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                ModalOutcome::Submit(ModalResult::Confirmed(self.action.clone()))
            }
            KeyCode::Char('n') | KeyCode::Char('N') => ModalOutcome::Close,
            KeyCode::Enter => {
                if self.confirm_focused {
                    ModalOutcome::Submit(ModalResult::Confirmed(self.action.clone()))
                } else {
                    ModalOutcome::Close
                }
            }
            _ => ModalOutcome::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEventKind;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: ratatui::crossterm::event::KeyEventState::NONE,
        }
    }

    fn typed(text: &str) -> Vec<KeyEvent> {
        text.chars().map(|c| key(KeyCode::Char(c))).collect()
    }

    #[test]
    fn escape_closes_every_modal() {
        let mut modals: Vec<Modal> = vec![
            Modal::AddRepository(AddRepositoryModal::new()),
            Modal::Confirm(ConfirmModal::new(
                "t",
                "m",
                ConfirmAction::RemoveRepository(Uuid::new_v4()),
            )),
            Modal::Settings(Box::new(SettingsModal::new(&Settings::default()))),
            Modal::CustomButtons(CustomButtonsModal::new(vec![])),
            Modal::EditButton(Box::new(EditButtonModal::new(None, &[], None))),
        ];
        for modal in &mut modals {
            assert!(matches!(
                modal.handle_key(key(KeyCode::Esc)),
                ModalOutcome::Close
            ));
        }
    }

    #[test]
    fn add_repository_requires_a_git_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut modal = AddRepositoryModal::new();
        modal
            .path
            .set_value(dir.path().to_string_lossy().to_string());
        assert_eq!(modal.status().0, "Not a git repository");
        assert!(matches!(
            modal.handle_key(key(KeyCode::Enter)),
            ModalOutcome::None
        ));

        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert!(modal.status().1);
        assert!(matches!(
            modal.handle_key(key(KeyCode::Enter)),
            ModalOutcome::Submit(ModalResult::RepositoryAdded { .. })
        ));
    }

    #[test]
    fn add_repository_toggles_import_checkbox() {
        let mut modal = AddRepositoryModal::new();
        modal.focus = 1;
        modal.handle_key(key(KeyCode::Char(' ')));
        assert!(modal.import_worktrees);
    }

    #[test]
    fn add_worktree_autofills_branch_from_name() {
        let repo = Repository::new("demo".into(), "/tmp/demo".into());
        let mut modal = AddWorktreeModal::new(
            &repo,
            vec!["main".into()],
            vec![],
            PathBuf::from("/forest"),
            "feat/".into(),
        );
        for k in typed("my feature!") {
            modal.handle_key(k);
        }
        assert_eq!(modal.sanitized_name(), "myfeature");
        assert_eq!(modal.branch.value(), "feat/myfeature");
        assert_eq!(
            modal.path_preview().unwrap(),
            PathBuf::from("/forest/demo/myfeature")
        );
    }

    #[test]
    fn add_worktree_rejects_unknown_existing_branch() {
        let repo = Repository::new("demo".into(), "/tmp/demo".into());
        let mut modal = AddWorktreeModal::new(
            &repo,
            vec!["main".into()],
            vec![],
            PathBuf::from("/forest"),
            "feat/".into(),
        );
        modal.name.set_value("wt");
        modal.new_branch = false;
        modal.search.set_value("nope");
        assert!(!modal.can_create());
        modal.focus = modal.create_index();
        modal.handle_key(key(KeyCode::Enter));
        assert!(modal.error.contains("does not exist"));

        modal.search.set_value("main");
        assert!(modal.can_create());
        assert!(matches!(
            modal.handle_key(key(KeyCode::Enter)),
            ModalOutcome::Submit(ModalResult::WorktreeCreated { .. })
        ));
    }

    #[test]
    fn edit_button_prefix_follows_label_until_edited() {
        let mut modal = EditButtonModal::new(None, &[], None);
        for k in typed("Yolo Disc") {
            modal.handle_key(k);
        }
        assert_eq!(modal.prefix.value(), "yolo-disc");

        modal.focus = 1;
        modal.handle_key(key(KeyCode::Backspace));
        assert!(!modal.follows);

        modal.focus = 0;
        modal.handle_key(key(KeyCode::Char('X')));
        assert_eq!(modal.prefix.value(), "yolo-dis");
    }

    #[test]
    fn edit_button_rejects_duplicates_and_empty_command() {
        let existing = vec![CustomClaudeButton {
            label: "Opus".into(),
            prefix: "opus".into(),
            command: "claude --model opus".into(),
        }];
        let mut modal = EditButtonModal::new(None, &existing, None);
        modal.label.set_value("Opus");
        modal.prefix.set_value("other");
        modal.focus = 3;
        modal.handle_key(key(KeyCode::Enter));
        assert_eq!(modal.error, "Command cannot be empty");

        modal.command.set_value("claude");
        modal.handle_key(key(KeyCode::Enter));
        assert!(modal.error.contains("label"));

        modal.label.set_value("Fresh");
        assert!(matches!(
            modal.handle_key(key(KeyCode::Enter)),
            ModalOutcome::Submit(ModalResult::CustomButtonSaved(_))
        ));
    }

    #[test]
    fn custom_buttons_reorder_and_delete() {
        let make = |name: &str| CustomClaudeButton {
            label: name.into(),
            prefix: name.to_lowercase(),
            command: "claude".into(),
        };
        let mut modal = CustomButtonsModal::new(vec![make("A"), make("B"), make("C")]);
        modal.selected = 2;
        modal.handle_key(key(KeyCode::Char('K')));
        assert_eq!(modal.buttons[1].label, "C");
        assert_eq!(modal.selected, 1);

        modal.handle_key(key(KeyCode::Char('d')));
        assert_eq!(modal.buttons.len(), 2);
        assert!(matches!(
            modal.handle_key(key(KeyCode::Char('s'))),
            ModalOutcome::Submit(ModalResult::CustomButtonsSaved(_))
        ));
    }

    #[test]
    fn child_results_flow_to_the_parent() {
        let mut settings = Modal::Settings(Box::new(SettingsModal::new(&Settings::default())));
        let button = CustomClaudeButton {
            label: "Opus".into(),
            prefix: "opus".into(),
            command: "claude --model opus".into(),
        };
        settings.receive_child(&ModalResult::CustomButtonsSaved(vec![button.clone()]));
        if let Modal::Settings(m) = &settings {
            assert_eq!(m.custom_buttons.len(), 1);
            assert_eq!(m.buttons_summary(), "1 custom button configured");
        } else {
            panic!("wrong modal");
        }

        let mut parent = Modal::CustomButtons(CustomButtonsModal::new(vec![]));
        parent.receive_child(&ModalResult::CustomButtonSaved(Box::new(button)));
        if let Modal::CustomButtons(m) = &parent {
            assert_eq!(m.buttons.len(), 1);
        } else {
            panic!("wrong modal");
        }
    }

    #[test]
    fn settings_cycles_selects_and_saves() {
        let mut modal = SettingsModal::new(&Settings::default());
        let start = modal.editor_index;
        modal.handle_key(key(KeyCode::Right));
        assert_ne!(modal.editor_index, start);
        modal.handle_key(key(KeyCode::Left));
        assert_eq!(modal.editor_index, start);

        modal.focus = 4;
        match modal.handle_key(key(KeyCode::Enter)) {
            ModalOutcome::Submit(ModalResult::SettingsSaved(s)) => {
                assert_eq!(s.default_editor, EDITORS[modal.editor_index].1);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn confirm_needs_an_explicit_yes() {
        let mut modal = ConfirmModal::new(
            "Delete",
            "sure?",
            ConfirmAction::DeleteWorktree(Uuid::new_v4()),
        );
        // Enter defaults to Cancel.
        assert!(matches!(
            modal.handle_key(key(KeyCode::Enter)),
            ModalOutcome::Close
        ));
        assert!(matches!(
            modal.handle_key(key(KeyCode::Char('y'))),
            ModalOutcome::Submit(ModalResult::Confirmed(_))
        ));
    }

    #[test]
    fn base_branch_default_prefers_remote() {
        let branches = vec!["main".to_string(), "origin/main".to_string()];
        let remotes = vec!["origin".to_string()];
        assert_eq!(
            default_base_branch(&branches, &remotes, "main"),
            "origin/main"
        );
        assert_eq!(default_base_branch(&branches, &[], "main"), "main");
        assert_eq!(default_base_branch(&branches, &[], "nope"), "main");
        assert_eq!(default_base_branch(&[], &[], "main"), "");
    }
}
