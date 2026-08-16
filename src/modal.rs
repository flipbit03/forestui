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
use ratatui::crossterm::event::{KeyCode, KeyEvent};
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
    /// Second-step confirm: the worktree was seen to hold uncommitted work.
    ForceDeleteWorktree(Uuid),
    RemoveRepository(Uuid),
}

impl ConfirmAction {
    /// Label on the destructive button. Derived from the action so a
    /// construction site cannot pair a discard-your-work action with a bland
    /// "Delete" button.
    pub fn confirm_label(&self) -> &'static str {
        match self {
            ConfirmAction::ForceDeleteWorktree(_) => "Delete anyway",
            ConfirmAction::DeleteWorktree(_) | ConfirmAction::RemoveRepository(_) => "Delete",
        }
    }
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
    /// The theme picker committed a slug; the Settings modal underneath
    /// carries it into the settings it will save. `&'static` because every
    /// slug lives in `theme::THEMES` — a non-theme value is unrepresentable.
    ThemeChosen(&'static str),
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
    ThemePicker(ThemePickerModal),
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
            Modal::ThemePicker(m) => m.handle_key(key),
            Modal::Confirm(m) => m.handle_key(key),
        }
    }

    /// Deliver a nested modal's result to its parent.
    pub fn receive_child(&mut self, result: &ModalResult) {
        match (self, result) {
            (Modal::Settings(parent), ModalResult::CustomButtonsSaved(buttons)) => {
                parent.custom_buttons = buttons.clone();
            }
            (Modal::Settings(parent), ModalResult::ThemeChosen(slug)) => {
                parent.theme_slug = slug;
            }
            (Modal::CustomButtons(parent), ModalResult::CustomButtonSaved(button)) => {
                parent.apply_edit((**button).clone());
            }
            _ => {}
        }
    }

    /// Move this modal's focus to `index`, as a click on that control does.
    ///
    /// The list-style modals do not have a focus ring, so the index means the
    /// selected row for Custom Buttons and the choice for Confirm.
    pub fn set_focus(&mut self, index: usize) {
        match self {
            Modal::AddRepository(m) => m.focus = index.min(AddRepositoryModal::FIELDS - 1),
            Modal::AddWorktree(m) => m.focus = index.min(m.field_count() - 1),
            Modal::CreateFromIssue(m) => m.focus = index.min(CreateFromIssueModal::FIELDS - 1),
            Modal::Settings(m) => m.focus = index.min(SettingsModal::FIELDS - 1),
            Modal::EditButton(m) => m.focus = index.min(EditButtonModal::FIELDS - 1),
            Modal::ThemePicker(m) => m.select(index),
            Modal::CustomButtons(m) => {
                if !m.buttons.is_empty() {
                    m.selected = index.min(m.buttons.len() - 1);
                }
            }
            // Confirm has two choices rather than a ring: 0 Cancel, 1 Delete.
            Modal::Confirm(m) => m.confirm_focused = index == 1,
        }
    }

    /// Select a row in whichever list this modal shows, for a click on it.
    pub fn set_row(&mut self, row: usize) {
        match self {
            Modal::AddWorktree(m) => {
                let count = m.matches().len();
                if count > 0 {
                    m.search_index = row.min(count - 1);
                    // Picking a row commits it, the way Enter does from the
                    // keyboard. Only `search` is read by `selected_branch` and
                    // `can_create`, so highlighting alone left Create disabled
                    // and a mouse-only user with no way to choose a branch.
                    if let Some((branch, _)) = m.matches().into_iter().nth(m.search_index) {
                        m.search.set_value(branch);
                    }
                }
            }
            Modal::CustomButtons(m) if !m.buttons.is_empty() => {
                m.selected = row.min(m.buttons.len() - 1);
            }
            Modal::ThemePicker(m) => m.select(row),
            _ => {}
        }
    }

    /// Advance any spinner this modal owns. Returns whether something visible
    /// moved, so the caller knows the frame needs repainting.
    pub fn tick(&mut self) -> bool {
        if let Modal::CreateFromIssue(m) = self
            && m.is_fetching
        {
            m.spinner_index = (m.spinner_index + 1) % SPINNER.len();
            return true;
        }
        false
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
    input.apply_edit_key(key)
}

// ---------------------------------------------------------------- Add repository

#[derive(Debug)]
pub struct AddRepositoryModal {
    pub path: TextInput,
    pub import_worktrees: bool,
    /// One of the `FOCUS_*` constants; see [`AddRepositoryModal`].
    pub focus: usize,
}

impl Default for AddRepositoryModal {
    fn default() -> Self {
        Self::new()
    }
}

impl AddRepositoryModal {
    /// The focus ring, in the order `ui/modals.rs` draws it. These are the
    /// single definition of the contract between the two files: the renderer
    /// records a click region against the same constant `handle_key` matches on.
    pub const FOCUS_PATH: usize = 0;
    pub const FOCUS_IMPORT: usize = 1;
    pub const FOCUS_ADD: usize = 2;
    pub const FOCUS_CANCEL: usize = 3;
    pub const FIELDS: usize = Self::FOCUS_CANCEL + 1;

    pub fn new() -> Self {
        Self {
            path: TextInput::new("").with_placeholder("Enter path or paste from clipboard..."),
            import_worktrees: false,
            focus: Self::FOCUS_PATH,
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
        if self.focus == Self::FOCUS_PATH && edit_input(&mut self.path, key) {
            return ModalOutcome::None;
        }
        if cycle_focus(&mut self.focus, Self::FIELDS, key) {
            return ModalOutcome::None;
        }
        match (self.focus, key.code) {
            (Self::FOCUS_IMPORT, KeyCode::Char(' ')) | (Self::FOCUS_IMPORT, KeyCode::Enter) => {
                self.import_worktrees = !self.import_worktrees;
                ModalOutcome::None
            }
            (Self::FOCUS_PATH | Self::FOCUS_ADD, KeyCode::Enter) => self.submit(),
            (Self::FOCUS_CANCEL, KeyCode::Enter) => ModalOutcome::Close,
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
    /// One of the `FOCUS_*` constants, or [`AddWorktreeModal::create_index`] /
    /// [`AddWorktreeModal::cancel_index`], whose positions depend on the mode.
    pub focus: usize,
}

impl AddWorktreeModal {
    /// The fixed head of the focus ring. Create and Cancel follow, but their
    /// indices move with the mode — the results list only exists while an
    /// existing branch is being picked — so they are methods, not constants.
    pub const FOCUS_NAME: usize = 0;
    pub const FOCUS_MODE: usize = 1;
    pub const FOCUS_BRANCH: usize = 2;
    pub const FOCUS_RESULTS: usize = 3;

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
            focus: Self::FOCUS_NAME,
        }
    }

    pub fn field_count(&self) -> usize {
        // The fixed head, then Create and Cancel; the results list sits between
        // them only while an existing branch is being picked.
        let head = Self::FOCUS_BRANCH + 1;
        head + usize::from(!self.new_branch) + 2
    }

    pub fn create_index(&self) -> usize {
        self.field_count() - 2
    }

    pub fn cancel_index(&self) -> usize {
        self.field_count() - 1
    }

    /// Index of the results list, which only exists in existing-branch mode.
    fn results_index(&self) -> Option<usize> {
        if self.new_branch {
            None
        } else {
            Some(Self::FOCUS_RESULTS)
        }
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
        if self.focus == Self::FOCUS_NAME && edit_input(&mut self.name, key) {
            self.error.clear();
            // A new branch name tracks the worktree name.
            if self.new_branch {
                self.branch
                    .set_value(format!("{}{}", self.branch_prefix, self.sanitized_name()));
            }
            return ModalOutcome::None;
        }
        if self.focus == Self::FOCUS_BRANCH {
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

        if self.focus == Self::FOCUS_MODE {
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
            if self.focus == self.create_index()
                || self.focus == Self::FOCUS_NAME
                || self.focus == Self::FOCUS_BRANCH
            {
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
    /// One of the `FOCUS_*` constants; see [`CreateFromIssueModal`].
    pub focus: usize,
}

impl CreateFromIssueModal {
    /// The focus ring, in the order `ui/modals.rs` draws it.
    pub const FOCUS_NAME: usize = 0;
    pub const FOCUS_BRANCH: usize = 1;
    pub const FOCUS_BASE: usize = 2;
    pub const FOCUS_FETCH: usize = 3;
    pub const FOCUS_PULL: usize = 4;
    pub const FOCUS_CREATE: usize = 5;
    pub const FOCUS_CANCEL: usize = 6;
    pub const FIELDS: usize = Self::FOCUS_CANCEL + 1;

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
            focus: Self::FOCUS_NAME,
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
            Self::FOCUS_NAME => Some(&mut self.name),
            Self::FOCUS_BRANCH => Some(&mut self.branch),
            Self::FOCUS_BASE => Some(&mut self.base_branch),
            _ => None,
        };
        if let Some(input) = editable
            && edit_input(input, key)
        {
            return ModalOutcome::None;
        }

        if self.focus == Self::FOCUS_BASE
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
            (Self::FOCUS_FETCH, KeyCode::Enter) => {
                if self.is_fetching {
                    return ModalOutcome::None;
                }
                self.is_fetching = true;
                self.spinner_index = 0;
                ModalOutcome::Effect(ModalEffect::Fetch(self.repo_path.clone()))
            }
            (Self::FOCUS_PULL, KeyCode::Enter) | (Self::FOCUS_PULL, KeyCode::Char(' ')) => {
                self.pull_first = !self.pull_first;
                ModalOutcome::None
            }
            (Self::FOCUS_CANCEL, KeyCode::Enter) => ModalOutcome::Close,
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
    /// Slug of the theme this dialog will save. The *active* theme may run
    /// ahead of it while the picker previews; Save persists this, Cancel
    /// restores the theme the dialog opened with.
    pub theme_slug: &'static str,
    /// What was active when the dialog opened, for the Cancel path.
    opened_with_theme: &'static str,
    /// The Python build's inert theme value, written back untouched — its
    /// Settings dialog crashes on anything outside its own option list.
    legacy_theme: String,
    pub branch_prefix: TextInput,
    pub custom_buttons: Vec<CustomClaudeButton>,
    /// One of the `FOCUS_*` constants; see [`SettingsModal`].
    pub focus: usize,
}

impl SettingsModal {
    /// The focus ring, in the order `ui/modals.rs` draws it.
    pub const FOCUS_EDITOR: usize = 0;
    pub const FOCUS_PREFIX: usize = 1;
    pub const FOCUS_THEME: usize = 2;
    pub const FOCUS_MANAGE: usize = 3;
    pub const FOCUS_SAVE: usize = 4;
    pub const FOCUS_CANCEL: usize = 5;
    pub const FIELDS: usize = Self::FOCUS_CANCEL + 1;

    pub fn new(settings: &Settings) -> Self {
        // Resolve an unknown slug to the default once, here, so the dialog
        // and the picker only ever see valid themes.
        let theme_slug = crate::theme::by_slug(&settings.theme_name)
            .unwrap_or(&crate::theme::THEMES[0])
            .slug;
        Self {
            editor_index: EDITORS
                .iter()
                .position(|(_, cmd)| *cmd == settings.default_editor)
                .unwrap_or(0),
            opened_with_theme: theme_slug,
            theme_slug,
            legacy_theme: settings.legacy_theme.clone(),
            branch_prefix: TextInput::new(settings.branch_prefix.clone()).with_placeholder("feat/"),
            custom_buttons: settings.custom_buttons.clone(),
            focus: Self::FOCUS_EDITOR,
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
            legacy_theme: self.legacy_theme.clone(),
            theme_name: self.theme_slug.to_string(),
            custom_buttons: self.custom_buttons.clone(),
        }
    }

    /// Closing without saving abandons any theme the picker applied — the
    /// live preview must not outlive the dialog it was previewed in.
    fn close_without_saving(&self) -> ModalOutcome {
        crate::theme::set_active(self.opened_with_theme);
        ModalOutcome::Close
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return self.close_without_saving();
        }
        if self.focus == Self::FOCUS_PREFIX && edit_input(&mut self.branch_prefix, key) {
            return ModalOutcome::None;
        }

        match (self.focus, key.code) {
            (Self::FOCUS_EDITOR, KeyCode::Left) => {
                self.editor_index = wrap_dec(self.editor_index, EDITORS.len());
                return ModalOutcome::None;
            }
            (Self::FOCUS_EDITOR, KeyCode::Right) => {
                self.editor_index = (self.editor_index + 1) % EDITORS.len();
                return ModalOutcome::None;
            }
            _ => {}
        }

        if cycle_focus(&mut self.focus, Self::FIELDS, key) {
            return ModalOutcome::None;
        }

        match (self.focus, key.code) {
            (Self::FOCUS_THEME, KeyCode::Enter) => {
                ModalOutcome::Push(Box::new(Modal::ThemePicker(ThemePickerModal::new())))
            }
            (Self::FOCUS_MANAGE, KeyCode::Enter) => ModalOutcome::Push(Box::new(
                Modal::CustomButtons(CustomButtonsModal::new(self.custom_buttons.clone())),
            )),
            (Self::FOCUS_CANCEL, KeyCode::Enter) => self.close_without_saving(),
            (_, KeyCode::Enter) => {
                ModalOutcome::Submit(ModalResult::SettingsSaved(Box::new(self.to_settings())))
            }
            _ => ModalOutcome::None,
        }
    }
}

// ------------------------------------------------------------------ Theme picker

/// Scrollable list over [`crate::theme::THEMES`]. Moving the highlight applies
/// the candidate theme to the whole app immediately — the panes behind the
/// dialog are the preview — and Esc puts back what was active when the picker
/// opened. Enter commits the slug to the Settings dialog underneath, which
/// still has to be saved to persist it.
#[derive(Debug)]
pub struct ThemePickerModal {
    /// Highlighted row, an index into [`crate::theme::THEMES`].
    pub index: usize,
    /// Active slug when the picker opened, restored on Esc.
    opened_with: &'static str,
}

impl ThemePickerModal {
    pub fn new() -> Self {
        let opened_with = crate::theme::active().slug;
        Self {
            index: crate::theme::THEMES
                .iter()
                .position(|t| t.slug == opened_with)
                .unwrap_or(0),
            opened_with,
        }
    }

    /// Move the highlight and apply the candidate, clamped to the list.
    pub fn select(&mut self, index: usize) {
        self.index = index.min(crate::theme::THEMES.len() - 1);
        crate::theme::set_active(crate::theme::THEMES[self.index].slug);
    }

    /// One step in either direction, for the wheel: unlike the arrow keys it
    /// does not wrap, so scrolling to an end stops there the way every
    /// scrolled list does.
    pub fn step(&mut self, delta: isize) {
        let stepped = self.index.saturating_add_signed(delta);
        self.select(stepped);
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            crate::theme::set_active(self.opened_with);
            return ModalOutcome::Close;
        }
        let len = crate::theme::THEMES.len();
        match key.code {
            KeyCode::Up | KeyCode::BackTab => {
                self.select(wrap_dec(self.index, len));
                ModalOutcome::None
            }
            KeyCode::Down | KeyCode::Tab => {
                self.select((self.index + 1) % len);
                ModalOutcome::None
            }
            KeyCode::Home => {
                self.select(0);
                ModalOutcome::None
            }
            KeyCode::End => {
                self.select(len - 1);
                ModalOutcome::None
            }
            KeyCode::Enter => ModalOutcome::Submit(ModalResult::ThemeChosen(
                crate::theme::THEMES[self.index].slug,
            )),
            _ => ModalOutcome::None,
        }
    }
}

impl Default for ThemePickerModal {
    fn default() -> Self {
        Self::new()
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
    /// One of the `FOCUS_*` constants; see [`EditButtonModal`].
    pub focus: usize,
}

impl EditButtonModal {
    /// The focus ring, in the order `ui/modals.rs` draws it.
    pub const FOCUS_LABEL: usize = 0;
    pub const FOCUS_PREFIX: usize = 1;
    pub const FOCUS_COMMAND: usize = 2;
    pub const FOCUS_SAVE: usize = 3;
    pub const FOCUS_CANCEL: usize = 4;
    pub const FIELDS: usize = Self::FOCUS_CANCEL + 1;

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
            focus: Self::FOCUS_LABEL,
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
            Self::FOCUS_LABEL => {
                if edit_input(&mut self.label, key) {
                    if self.follows {
                        self.prefix.set_value(derive_prefix(self.label.value()));
                    }
                    return ModalOutcome::None;
                }
            }
            Self::FOCUS_PREFIX => {
                if edit_input(&mut self.prefix, key) {
                    self.follows = self.prefix.value() == derive_prefix(self.label.value());
                    return ModalOutcome::None;
                }
            }
            Self::FOCUS_COMMAND if edit_input(&mut self.command, key) => return ModalOutcome::None,
            _ => {}
        }

        if cycle_focus(&mut self.focus, Self::FIELDS, key) {
            return ModalOutcome::None;
        }

        match (self.focus, key.code) {
            (Self::FOCUS_CANCEL, KeyCode::Enter) => ModalOutcome::Close,
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
    /// Submit keys are ignored until this instant. Set on confirms pushed by a
    /// *background event*: a keystroke queued before the modal appeared (a `y`
    /// aimed at ClaudeYolo, letters typed into the modal underneath) must not
    /// confirm a dialog the user never saw. Cancel always works.
    armed_at: Option<std::time::Instant>,
}

/// Long enough to outlive any keystroke already in the event queue, far too
/// short for a human deliberately confirming to notice.
const CONFIRM_ARM_DELAY: std::time::Duration = std::time::Duration::from_millis(400);

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
            armed_at: None,
        }
    }

    /// For confirms pushed by a background event rather than a user action.
    pub fn with_arm_delay(mut self) -> Self {
        self.armed_at = Some(std::time::Instant::now() + CONFIRM_ARM_DELAY);
        self
    }

    #[cfg(test)]
    pub fn disarm(&mut self) {
        self.armed_at = None;
    }

    fn submit_armed(&self) -> bool {
        self.armed_at
            .is_none_or(|at| std::time::Instant::now() >= at)
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
                if self.submit_armed() {
                    ModalOutcome::Submit(ModalResult::Confirmed(self.action.clone()))
                } else {
                    ModalOutcome::None
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => ModalOutcome::Close,
            KeyCode::Enter => {
                if self.confirm_focused && self.submit_armed() {
                    ModalOutcome::Submit(ModalResult::Confirmed(self.action.clone()))
                } else if !self.confirm_focused {
                    ModalOutcome::Close
                } else {
                    ModalOutcome::None
                }
            }
            _ => ModalOutcome::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyModifiers};

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
        modal.focus = AddRepositoryModal::FOCUS_IMPORT;
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

    /// Clicking a branch row has to *pick* the branch, not just light it up:
    /// `can_create` reads `search`, so a mouse-only user was left with Create
    /// permanently disabled and no way to choose.
    #[test]
    fn clicking_a_branch_row_selects_that_branch() {
        let repo = Repository::new("demo".into(), "/tmp/demo".into());
        let mut modal = Modal::AddWorktree(Box::new(AddWorktreeModal::new(
            &repo,
            vec!["main".into(), "feat/login".into()],
            vec![],
            PathBuf::from("/forest"),
            "feat/".into(),
        )));
        let expected = if let Modal::AddWorktree(m) = &mut modal {
            m.name.set_value("wt");
            m.new_branch = false;
            assert!(!m.can_create(), "nothing is picked yet");
            m.matches()[1].0.clone()
        } else {
            panic!("add worktree modal expected")
        };

        modal.set_row(1);

        let Modal::AddWorktree(m) = &modal else {
            panic!("add worktree modal expected")
        };
        assert_eq!(m.selected_branch(), expected);
        assert!(m.can_create(), "the clicked branch was never committed");
    }

    #[test]
    fn edit_button_prefix_follows_label_until_edited() {
        let mut modal = EditButtonModal::new(None, &[], None);
        for k in typed("Yolo Disc") {
            modal.handle_key(k);
        }
        assert_eq!(modal.prefix.value(), "yolo-disc");

        modal.focus = EditButtonModal::FOCUS_PREFIX;
        modal.handle_key(key(KeyCode::Backspace));
        assert!(!modal.follows);

        modal.focus = EditButtonModal::FOCUS_LABEL;
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
        modal.focus = EditButtonModal::FOCUS_SAVE;
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

        modal.focus = SettingsModal::FOCUS_SAVE;
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

    /// The picker's contract: moving the highlight applies the candidate to
    /// the whole app, Esc restores what was active when it opened, and Enter
    /// commits the slug while leaving the preview in place.
    #[test]
    fn theme_picker_previews_commits_and_reverts() {
        let _guard = crate::theme::test_lock();
        crate::theme::set_active("forest-dark");

        let mut picker = ThemePickerModal::new();
        picker.handle_key(key(KeyCode::Down));
        assert_eq!(crate::theme::active().slug, crate::theme::THEMES[1].slug);
        assert!(matches!(
            picker.handle_key(key(KeyCode::Esc)),
            ModalOutcome::Close
        ));
        assert_eq!(crate::theme::active().slug, "forest-dark");

        let mut picker = ThemePickerModal::new();
        picker.handle_key(key(KeyCode::Down));
        let ModalOutcome::Submit(ModalResult::ThemeChosen(slug)) =
            picker.handle_key(key(KeyCode::Enter))
        else {
            panic!("Enter must commit the highlighted theme");
        };
        assert_eq!(slug, crate::theme::THEMES[1].slug);
        assert_eq!(crate::theme::active().slug, slug);
        // The guard restores the pre-test theme on drop.
    }

    /// A previewed theme must not outlive the dialog it was previewed in:
    /// closing Settings without saving restores the theme it opened with.
    #[test]
    fn cancelling_settings_abandons_a_previewed_theme() {
        let _guard = crate::theme::test_lock();
        crate::theme::set_active("forest-dark");

        let mut settings = SettingsModal::new(&Settings::default());
        // The picker previewed and committed a different slug into the dialog.
        crate::theme::set_active("dracula");
        settings.theme_slug = "dracula";

        assert!(matches!(
            settings.handle_key(key(KeyCode::Esc)),
            ModalOutcome::Close
        ));
        assert_eq!(crate::theme::active().slug, "forest-dark");
    }

    #[test]
    fn saving_settings_carries_the_chosen_theme() {
        let _guard = crate::theme::test_lock();
        let mut settings = SettingsModal::new(&Settings::default());
        settings.theme_slug = "nord";
        settings.focus = SettingsModal::FOCUS_SAVE;

        let ModalOutcome::Submit(ModalResult::SettingsSaved(saved)) =
            settings.handle_key(key(KeyCode::Enter))
        else {
            panic!("Save must submit the settings");
        };
        assert_eq!(saved.theme_name, "nord");
        // The legacy field is written back untouched — the Python build's
        // Settings dialog crashes on values outside System/Dark/Light.
        assert_eq!(saved.legacy_theme, "system");
    }

    /// An unknown stored slug resolves to the default when the dialog opens,
    /// so a hand-edited or future-versioned file neither crashes nor persists
    /// an unknown value forward.
    #[test]
    fn unknown_theme_names_resolve_to_the_default_slug() {
        let settings = Settings {
            theme_name: "some-future-theme".into(),
            ..Settings::default()
        };
        let modal = SettingsModal::new(&settings);
        assert_eq!(modal.theme_slug, "forest-dark");
    }
}
