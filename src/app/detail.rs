//! What the detail pane contains, independent of how it is drawn.
//!
//! This is the single source of truth for the pane. [`content`] walks the
//! current selection and produces the full sequence of nodes — headers, text,
//! cards, and every focusable control in order — and *both* consumers derive
//! from it: `App::detail_items` collects the focusable items for key handling,
//! and `ui/detail.rs` renders the nodes one by one, claiming a slot per
//! control as it goes. The two used to be written out twice, one list in each
//! file, with a comment demanding they be kept in the same order by hand;
//! deriving them from one walk makes that drift impossible rather than merely
//! tested against.

use super::App;
use crate::models::Speaker;
use crate::theme;
use ratatui::style::Style;

/// An actionable control in the detail pane — the immediate-mode stand-in for
/// Textual's buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Sync,
    AddWorktree,
    Editor,
    Terminal,
    Files,
    ClaudeNew,
    ClaudeYolo,
    ClaudeCustom(usize),
    ResumeSession(usize),
    ResumeYolo(usize),
    ResumeCustom { button: usize, session: usize },
    RenameSession(usize),
    TogglePinSession(usize),
    DeleteSession(usize),
    RefreshIssues,
    CreateFromIssue(usize),
    RemoveRepository,
    Archive,
    Unarchive,
    Delete,
}

/// An editable field in the detail pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    WorktreeName,
    BranchName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailItem {
    Action(Action),
    Field(Field),
}

/// One boxed control inside a [`DetailNode::Controls`] row.
#[derive(Debug, Clone)]
pub struct ControlSpec {
    /// What activating this control means — also its entry in `detail_items`.
    pub item: DetailItem,
    pub label: String,
    pub variant: theme::Variant,
    /// A control that cannot run still occupies its slot, so the layout and
    /// the item indices never shift.
    pub enabled: bool,
}

impl ControlSpec {
    fn new(action: Action, label: impl Into<String>, variant: theme::Variant) -> Self {
        Self {
            item: DetailItem::Action(action),
            label: label.into(),
            variant,
            enabled: true,
        }
    }

    fn disabled(action: Action, label: impl Into<String>) -> Self {
        Self {
            enabled: false,
            ..Self::new(action, label, theme::Variant::Normal)
        }
    }
}

/// The header line the issues section shares with its refresh control.
pub const ISSUES_HEADER: &str = "MY OPEN GITHUB ISSUES";

/// One piece of the pane, in rendering order.
#[derive(Debug, Clone)]
pub enum DetailNode {
    /// Section header, with the unconditional blank row above it that
    /// Textual's `.section-header { margin: 1 0 0 0 }` produced.
    Section(&'static str),
    Text {
        text: String,
        style: Style,
    },
    /// One line built from differently styled pieces — the session info line
    /// needs its live badge coloured apart from the rest.
    Spans(Vec<(String, Style)>),
    Blank,
    /// The horizontal rule between major sections, with its margin row above.
    Rule,
    /// A path in the elevated box that hugs its content.
    PathBox {
        path: String,
        style: Style,
    },
    /// Open a card; nodes until [`DetailNode::CardEnd`] land inside it.
    CardStart {
        padded: bool,
    },
    CardEnd,
    /// A row of boxed controls. `lead` is non-clickable text sharing the line.
    Controls {
        lead: Option<(String, Style)>,
        controls: Vec<ControlSpec>,
    },
    /// A rename field, rendered from the app's live input for `field`.
    Field {
        field: Field,
        label: &'static str,
    },
    /// The issues header with its inline refresh control, whose glyph doubles
    /// as the loading spinner.
    IssuesHeader {
        glyph: char,
    },
}

/// The full pane for the current selection. Empty when nothing is selected,
/// which is what the renderer shows the empty state for.
pub fn content(app: &App) -> Vec<DetailNode> {
    let mut nodes = Vec::new();
    if app.state.selection.is_worktree() {
        worktree(&mut nodes, app);
    } else if app.state.selection.is_repository() {
        repository(&mut nodes, app);
    }
    nodes
}

/// The focusable items a node sequence contains, in order. Test-only: the
/// running app consumes [`drawn`] via the `App::drawn_items` snapshot.
#[cfg(test)]
pub fn items(nodes: &[DetailNode]) -> Vec<DetailItem> {
    drawn(nodes).into_iter().map(|(item, _)| item).collect()
}

/// [`items`] paired with whether each control can actually run. This is what
/// the renderer snapshots onto `App::drawn_items`: activation resolves against
/// the frame the user saw, and a disabled control must not fire at all.
pub fn drawn(nodes: &[DetailNode]) -> Vec<(DetailItem, bool)> {
    let mut items = Vec::new();
    for node in nodes {
        match node {
            DetailNode::Controls { controls, .. } => {
                items.extend(
                    controls
                        .iter()
                        .map(|control| (control.item.clone(), control.enabled)),
                );
            }
            DetailNode::Field { field, .. } => items.push((DetailItem::Field(*field), true)),
            DetailNode::IssuesHeader { .. } => {
                items.push((DetailItem::Action(Action::RefreshIssues), true));
            }
            _ => {}
        }
    }
    items
}

fn text(nodes: &mut Vec<DetailNode>, text: impl Into<String>, style: Style) {
    nodes.push(DetailNode::Text {
        text: text.into(),
        style,
    });
}

fn controls(nodes: &mut Vec<DetailNode>, controls: Vec<ControlSpec>) {
    nodes.push(DetailNode::Controls {
        lead: None,
        controls,
    });
}

// --------------------------------------------------------------------- panes

fn repository(nodes: &mut Vec<DetailNode>, app: &App) {
    let repository = app.state.selected_repository();
    let name = repository.map(|r| r.name.as_str()).unwrap_or_default();
    let path = repository
        .map(|r| r.source_path.as_str())
        .unwrap_or_default();

    nodes.push(DetailNode::Section("MAIN REPOSITORY"));
    text(nodes, format!("Repository: {name}"), theme::title());
    if let Some(branch) = &app.meta.branch {
        text(nodes, format!("Branch:     {branch}"), theme::accent());
    }
    commit_line(nodes, app);

    controls(
        nodes,
        vec![
            sync_control(app, false),
            ControlSpec::new(Action::AddWorktree, "Add Worktree", theme::Variant::Normal),
        ],
    );

    nodes.push(DetailNode::Rule);
    nodes.push(DetailNode::Section("LOCATION"));
    nodes.push(DetailNode::PathBox {
        path: path.to_string(),
        style: theme::secondary(),
    });

    nodes.push(DetailNode::Rule);
    open_in(nodes);
    // Textual ran CLAUDE straight into RECENT SESSIONS with no rule between them.
    nodes.push(DetailNode::Rule);
    claude(nodes, app);
    sessions(nodes, app);

    nodes.push(DetailNode::Rule);
    issues(nodes, app);

    nodes.push(DetailNode::Rule);
    nodes.push(DetailNode::Section("MANAGE"));
    controls(
        nodes,
        vec![ControlSpec::new(
            Action::RemoveRepository,
            "Remove Repository",
            theme::Variant::Destructive,
        )],
    );
    bottom_padding(nodes);
}

/// A resting line under the pane's last control — its bottom padding. Without
/// it, a fully scrolled pane presses the final button straight against the
/// footer bar.
fn bottom_padding(nodes: &mut Vec<DetailNode>) {
    nodes.push(DetailNode::Blank);
}

fn worktree(nodes: &mut Vec<DetailNode>, app: &App) {
    // Read defensively: a selection can briefly outlive the worktree it points
    // at, and the sequence of focusable items must not change when it does.
    let selected = app.state.selected_worktree();
    let repository = selected
        .map(|(repo, _)| repo.name.as_str())
        .unwrap_or_default();
    let name = selected.map(|(_, w)| w.name.as_str()).unwrap_or_default();
    let branch = selected.map(|(_, w)| w.branch.as_str()).unwrap_or_default();
    let archived = selected.map(|(_, w)| w.is_archived).unwrap_or(false);
    let deleting = selected
        .map(|(_, w)| app.removals_in_flight.contains(&w.id))
        .unwrap_or(false);

    nodes.push(DetailNode::Section("WORKTREE"));
    text(nodes, format!("Repository: {repository}"), theme::title());
    text(nodes, format!("Worktree:   {name}"), theme::primary());
    text(nodes, format!("Branch:     {branch}"), theme::accent());
    if !app.meta.path_exists {
        text(
            nodes,
            "⚠ MISSING:   directory no longer exists on disk",
            theme::destructive(),
        );
    }
    if let Some(base) = selected.and_then(|(_, w)| w.base_branch.as_deref()) {
        let mut line = format!("Based on:   {base}");
        if let Some(reference) = selected.and_then(|(_, w)| w.created_from_ref.as_deref()) {
            line.push_str(&format!(" ({reference})"));
        }
        text(nodes, line, theme::muted());
    }
    commit_line(nodes, app);

    controls(nodes, vec![sync_control(app, !app.meta.path_exists)]);

    nodes.push(DetailNode::Rule);
    nodes.push(DetailNode::Section("LOCATION"));
    if app.meta.path_exists {
        nodes.push(DetailNode::PathBox {
            path: app.meta.path.clone(),
            style: theme::secondary(),
        });
    } else {
        nodes.push(DetailNode::PathBox {
            path: format!("{}  (missing)", app.meta.path),
            style: theme::destructive(),
        });
    }

    nodes.push(DetailNode::Rule);
    open_in(nodes);
    nodes.push(DetailNode::Rule);
    claude(nodes, app);
    sessions(nodes, app);

    nodes.push(DetailNode::Rule);
    nodes.push(DetailNode::Section("RENAME"));
    nodes.push(DetailNode::Field {
        field: Field::WorktreeName,
        label: "Worktree name",
    });
    nodes.push(DetailNode::Field {
        field: Field::BranchName,
        label: "Branch name",
    });

    nodes.push(DetailNode::Rule);
    nodes.push(DetailNode::Section("MANAGE"));
    controls(
        nodes,
        vec![
            if archived {
                ControlSpec::new(Action::Unarchive, "Unarchive", theme::Variant::Normal)
            } else {
                ControlSpec::new(Action::Archive, "Archive", theme::Variant::Normal)
            },
            if deleting {
                ControlSpec::disabled(Action::Delete, "Deleting…")
            } else {
                ControlSpec::new(Action::Delete, "Delete", theme::Variant::Destructive)
            },
        ],
    );
    bottom_padding(nodes);
}

// ------------------------------------------------------------------ sections

fn commit_line(nodes: &mut Vec<DetailNode>, app: &App) {
    let Some(hash) = &app.meta.commit_hash else {
        return;
    };
    let mut line = format!("Commit:     {hash}");
    if let Some(when) = app.meta.commit_time {
        line.push_str(&format!(" ({})", crate::util::naturaltime(when)));
    }
    text(nodes, line, theme::muted());
}

/// The `↓ Git Pull` control. Disabled when there is no remote to pull from, or
/// — for worktrees — when the directory is gone.
///
/// The Textual build used `⟳` (U+27F3). It is a poor choice for a terminal: no
/// monospace font checked here carries it — not DejaVu Sans Mono, Liberation
/// Mono, Noto Mono, Ubuntu Mono, nor the JetBrains Mono the screenshot tooling
/// embeds — so it lands as a `.notdef` box wherever the terminal has no font
/// fallback to lean on. `↓` is in every one of them, is single-width rather than
/// ambiguous, and says the same thing more plainly: a pull brings work down.
fn sync_control(app: &App, missing_directory: bool) -> ControlSpec {
    if missing_directory {
        ControlSpec::disabled(Action::Sync, "↓ Git Pull (Directory missing)")
    } else if app.meta.has_remote {
        ControlSpec::new(Action::Sync, "↓ Git Pull", theme::Variant::Normal)
    } else {
        ControlSpec::disabled(Action::Sync, "↓ Git Pull (No remote)")
    }
}

fn open_in(nodes: &mut Vec<DetailNode>) {
    nodes.push(DetailNode::Section("OPEN IN"));
    controls(
        nodes,
        vec![
            ControlSpec::new(Action::Editor, "Editor", theme::Variant::Normal),
            ControlSpec::new(Action::Terminal, "Terminal", theme::Variant::Normal),
            ControlSpec::new(Action::Files, "Files", theme::Variant::Normal),
        ],
    );
}

fn claude(nodes: &mut Vec<DetailNode>, app: &App) {
    nodes.push(DetailNode::Section("CLAUDE"));
    let mut row = vec![
        ControlSpec::new(Action::ClaudeNew, "New Session", theme::Variant::Primary),
        ControlSpec::new(
            Action::ClaudeYolo,
            "New Session: YOLO",
            theme::Variant::Destructive,
        ),
    ];
    row.extend(custom_controls(app, Action::ClaudeCustom));
    controls(nodes, row);
}

/// The user's own Claude buttons, which follow both the new-session and the
/// resume controls. `action` maps a button index to the action it fires in
/// this particular row.
fn custom_controls<'a>(
    app: &'a App,
    action: impl Fn(usize) -> Action + 'a,
) -> impl Iterator<Item = ControlSpec> + 'a {
    app.settings
        .custom_buttons
        .iter()
        .enumerate()
        .map(move |(index, custom)| {
            ControlSpec::new(
                action(index),
                custom.label.as_str(),
                theme::Variant::claude(custom.is_yolo_style()),
            )
        })
}

/// Shared by the issue header and the per-session refresh indicator.
const SPINNER: [char; 4] = ['|', '/', '-', '\\'];

fn sessions(nodes: &mut Vec<DetailNode>, app: &App) {
    nodes.push(DetailNode::Section("RECENT SESSIONS"));
    let Some(list) = app.sessions.as_deref() else {
        text(nodes, "Loading...", theme::muted());
        return;
    };
    if list.is_empty() {
        text(nodes, "No sessions found", theme::muted());
        return;
    }
    let path = app.state.selected_path().unwrap_or_default();
    // Every card is spaced away from what precedes it — the header included,
    // which is Textual's `.session-item { margin: 0 2 1 0 }` seen from above.
    for (index, session) in list.iter().enumerate() {
        let live = app.live_sessions.get(&session.id);
        let pinned = app.state.is_pinned(&path, &session.id);
        nodes.push(DetailNode::Blank);
        // Unpadded: the card's own top and bottom blanks cost two rows per
        // session, and with five cards on screen that is ten rows of nothing.
        // The name carries the separation instead, where it does some work.
        nodes.push(DetailNode::CardStart { padded: false });
        // The title line carries everything that identifies the card: the pin
        // marker, the name, and the live badge — there is horizontal room to
        // spare, and a badge on its own line repeated the name for nothing.
        // `◆`, not `★`: the star is missing from JetBrains Mono and friends —
        // the same tofu problem that retired `⟳` — while the diamond is in
        // every monospace font the sync control's comment lists.
        let mut title_line: Vec<(String, Style)> = Vec::new();
        if pinned {
            title_line.push(("◆ ".to_string(), theme::accent()));
        }
        title_line.push((crate::util::truncate(&session.title, 60), theme::primary()));
        // A map hit means a claude process is running this session right now —
        // a window whose Claude exited never enters the map, because that
        // session is free again (see `services/live.rs`).
        if let Some(live) = live {
            title_line.push((" · ".to_string(), theme::muted()));
            match &live.place {
                crate::services::live::LivePlace::Window { window_name, .. } => {
                    // The window name and the session name are one string by
                    // design, so it is only worth printing when the two
                    // actually differ — a `:2`-uniquified window, or an
                    // unnamed session whose title is its first prompt while
                    // the window carries forestui's name.
                    let place = if window_name == &session.title {
                        String::new()
                    } else {
                        format!(" in {}", crate::util::truncate(window_name, 25))
                    };
                    title_line.push((format!("● live{place}"), theme::accent()));
                }
                // A heartbeat with no reachable window: another tmux session,
                // or no tmux at all. Live, but not somewhere to jump to.
                crate::services::live::LivePlace::Elsewhere { .. } => {
                    title_line.push(("● live elsewhere".to_string(), theme::accent()));
                }
            }
        }
        nodes.push(DetailNode::Spans(title_line));
        // The name is what you scan the list for, so it gets the whitespace —
        // everything below it reads as one block of detail about that name.
        nodes.push(DetailNode::Blank);
        // The exchange the conversation stopped on. A turn identical to the
        // name is not worth a line of its own — that happens on an unnamed
        // session, whose name *is* its first message.
        for turn in session
            .recent_turns
            .iter()
            .filter(|turn| turn.text != session.title)
        {
            let style = match turn.speaker {
                Speaker::User => theme::secondary(),
                Speaker::Claude => theme::muted(),
            };
            text(
                nodes,
                format!(
                    "{} {}",
                    turn.speaker.label(),
                    crate::util::truncate(&turn.text, 60)
                ),
                style,
            );
        }
        // A card being re-read says so on its meta line. Without it a
        // conversation that moved on while forestui was in the background just
        // sits at its old turn count and then changes with no explanation.
        let count = session.message_count;
        let msgs = if count == 1 { "msg" } else { "msgs" };
        let mut meta = if app.sessions_refreshing.contains(&session.id) {
            format!(
                "{} refreshing • {count} {msgs}",
                SPINNER[app.spinner_index % SPINNER.len()],
            )
        } else {
            format!("{} • {count} {msgs}", session.relative_time())
        };
        // The spend rides the meta line, next to the message count it
        // explains. Old minimal transcripts carry no usage and add nothing.
        if !session.tokens.is_zero() {
            use crate::services::claude_session as service;
            meta.push_str(&format!(
                " • {} in / {} out",
                service::fmt_tokens(session.tokens.total_in()),
                service::fmt_tokens(session.tokens.output),
            ));
            if let Some(cost) = service::cost_estimate(session.model.as_deref(), session.tokens) {
                meta.push_str(&format!(" • {}", service::fmt_cost(cost)));
            }
        }
        let mut row = vec![
            ControlSpec::new(
                Action::ResumeSession(index),
                "Resume",
                theme::Variant::Normal,
            ),
            ControlSpec::new(
                Action::ResumeYolo(index),
                "YOLO",
                theme::Variant::Destructive,
            ),
        ];
        row.extend(custom_controls(app, move |button| Action::ResumeCustom {
            button,
            session: index,
        }));
        row.push(ControlSpec::new(
            Action::RenameSession(index),
            "Rename",
            theme::Variant::Normal,
        ));
        row.push(ControlSpec::new(
            Action::TogglePinSession(index),
            if pinned { "Unpin" } else { "Pin" },
            theme::Variant::Normal,
        ));
        // Deleting a session that is open in a window would pull the
        // transcript out from under a running Claude; the control keeps its
        // slot so the row never shifts, but it cannot fire.
        row.push(if live.is_some() {
            ControlSpec::disabled(Action::DeleteSession(index), "Del")
        } else {
            ControlSpec::new(
                Action::DeleteSession(index),
                "Del",
                theme::Variant::Destructive,
            )
        });
        nodes.push(DetailNode::Controls {
            lead: None,
            controls: row,
        });
        // Branch and meta close the card, each on a line of its own *below*
        // the buttons: sharing the button row pushed Pin and Del off the
        // card's right edge at ordinary widths, and sitting above the buttons
        // crammed them against the turns. The button block is what separates
        // them from the conversation.
        if let Some(branch) = &session.git_branch {
            nodes.push(DetailNode::Spans(vec![
                ("on branch ".to_string(), theme::muted()),
                (crate::util::truncate(branch, 40), theme::accent()),
            ]));
        }
        text(nodes, meta, theme::muted());
        nodes.push(DetailNode::CardEnd);
    }
}

fn issues(nodes: &mut Vec<DetailNode>, app: &App) {
    // The refresh control rides the header line and doubles as the loading
    // spinner — Textual's `.refresh-btn` was flat, `height: 1; border: none`.
    nodes.push(DetailNode::Blank);
    nodes.push(DetailNode::IssuesHeader {
        glyph: match app.issues {
            Some(_) => '↻',
            None => SPINNER[app.spinner_index % SPINNER.len()],
        },
    });

    let Some(list) = app.issues.as_deref() else {
        text(nodes, "Loading...", theme::muted());
        return;
    };
    if list.is_empty() {
        text(nodes, "No issues found", theme::muted());
        return;
    }
    for (index, issue) in list.iter().enumerate() {
        nodes.push(DetailNode::Blank);
        nodes.push(DetailNode::CardStart { padded: true });
        text(
            nodes,
            format!(
                "#{} {}",
                issue.number,
                crate::util::truncate(&issue.title, 45)
            ),
            theme::primary(),
        );

        let mut meta = issue.relative_time();
        let labels: Vec<&str> = issue
            .labels
            .iter()
            .take(2)
            .map(|label| label.name.as_str())
            .collect();
        if !labels.is_empty() {
            meta.push_str(&format!(" • {}", labels.join(", ")));
        }
        nodes.push(DetailNode::Controls {
            lead: Some((format!("{meta}  "), theme::muted())),
            controls: vec![ControlSpec::new(
                Action::CreateFromIssue(index),
                "Create WT",
                theme::Variant::Normal,
            )],
        });
        nodes.push(DetailNode::CardEnd);
    }
}
