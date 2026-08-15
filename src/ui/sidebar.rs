//! The repository / worktree tree on the left.

use crate::app::{App, Focus, HitTarget, SidebarRow};
use crate::theme;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    // `#sidebar-header-box { height: 3; padding: 1 0 0 0; border-bottom: solid }`
    // — a blank row, the centred status, then the rule that closes the box.
    let [header_area, tree_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);

    // `#sidebar { border-right: solid }` runs the sidebar's whole height, header
    // box included, so it is drawn over `area` rather than just the tree.
    // Always the resting border colour: `#sidebar { border-right: solid $border }`
    // had no focus rule, and focus is already legible from the cursor row, which
    // is the thing that actually moves.
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(theme::border());
    let inner = block.inner(tree_area);
    let status_area = block.inner(header_area);
    frame.render_widget(block, area);

    draw_gh_status(frame, app, status_area);

    if app.rows.is_empty() {
        let lines = vec![
            Line::from(Span::styled("No repositories", theme::muted())),
            Line::from(Span::styled("Press [a] to add one", theme::muted())),
        ];
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let hovered = match app.hovered {
        Some(HitTarget::SidebarRow(index)) | Some(HitTarget::SidebarToggle(index)) => Some(index),
        _ => None,
    };
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| row_to_item(row, hovered == Some(index)))
        .collect();
    let mut state = ListState::default();
    state.select(Some(app.sidebar_index));

    let highlight = if app.focus == Focus::Sidebar {
        theme::cursor()
    } else {
        theme::cursor_unfocused()
    };
    frame.render_stateful_widget(
        List::new(items).highlight_style(highlight),
        inner,
        &mut state,
    );

    // The list decides its own scroll offset during render, so the clickable
    // rows can only be worked out afterwards.
    let offset = state.offset();
    for screen_row in 0..inner.height {
        let index = offset + screen_row as usize;
        if index >= app.rows.len() {
            break;
        }
        app.push_hit(
            Rect {
                x: inner.x,
                y: inner.y + screen_row,
                width: inner.width,
                height: 1,
            },
            HitTarget::SidebarRow(index),
        );
        // The twisty is pushed after the row so it wins the click: folding a
        // repository away must not also select it. Two cells wide, matching
        // the glyph and the space after it.
        if matches!(
            app.rows.get(index),
            Some(SidebarRow::Repository {
                has_worktrees: true,
                ..
            })
        ) {
            app.push_hit(
                Rect {
                    x: inner.x,
                    y: inner.y + screen_row,
                    width: TWISTY_WIDTH.min(inner.width),
                    height: 1,
                },
                HitTarget::SidebarToggle(index),
            );
        }
    }
}

/// Cells the `▾`/`▸` twisty and its trailing space occupy.
const TWISTY_WIDTH: u16 = 2;

fn draw_gh_status(frame: &mut Frame, app: &App, area: Rect) {
    let style = match app.gh_status.as_str() {
        s if s.starts_with("ok") => Style::default().fg(theme::SUCCESS),
        "unauth'd" => Style::default().fg(theme::WARNING),
        _ => Style::default().fg(theme::TEXT_MUTED),
    };
    let text = format!("gh cli: {}", app.gh_status);
    let padding = (area.width as usize).saturating_sub(text.chars().count()) / 2;
    frame.render_widget(
        Paragraph::new(vec![
            Line::default(),
            Line::from(vec![
                Span::raw(" ".repeat(padding)),
                Span::styled(text, style),
            ]),
            // The box's `border-bottom`, drawn across the sidebar and its divider
            // so the two borders meet rather than leaving a notch.
            Line::from(Span::styled(
                "─".repeat(area.width as usize),
                theme::border(),
            )),
        ])
        .style(Style::default().bg(theme::BG_ELEVATED)),
        area,
    );
}

fn row_to_item(row: &SidebarRow, hovered: bool) -> ListItem<'static> {
    // `ListItem`'s own style paints the whole row, so hover reads as a band
    // across the sidebar exactly as Textual's `ListItem:hover` did.
    let item = match row {
        SidebarRow::Repository {
            name,
            has_worktrees,
            collapsed,
            ..
        } => {
            // A repository with nothing under it gets a blank of the same width,
            // so every name still starts in the same column.
            let twisty = match (has_worktrees, collapsed) {
                (false, _) => "  ",
                (true, false) => "▾ ",
                (true, true) => "▸ ",
            };
            ListItem::new(Line::from(vec![
                Span::styled(twisty, theme::muted()),
                Span::styled(name.clone(), theme::title()),
            ]))
        }
        SidebarRow::Worktree {
            name,
            branch,
            is_last,
            ..
        } => {
            let prefix = if *is_last { "└─" } else { "├─" };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{prefix} {name} "), theme::primary()),
                Span::styled(format!("[{branch}]"), theme::accent()),
            ]))
        }
        SidebarRow::ArchivedHeader => ListItem::new(Line::from(Span::styled(
            " Archived",
            theme::section_header(),
        ))),
        SidebarRow::ArchivedWorktree {
            name, repo_name, ..
        } => ListItem::new(Line::from(Span::styled(
            format!("   {name} ({repo_name})"),
            theme::muted(),
        ))),
    };
    if hovered {
        item.style(Style::default().bg(theme::BG_HOVER))
    } else {
        item
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn worktree_rows_show_the_branch() {
        let row = SidebarRow::Worktree {
            repo_id: Uuid::new_v4(),
            id: Uuid::new_v4(),
            name: "wt-two".into(),
            branch: "feat/wt-two".into(),
            is_last: true,
        };
        let item = row_to_item(&row, false);
        let rendered: String = format!("{item:?}");
        // The Textual build lost this to console-markup parsing; assert it survives.
        assert!(rendered.contains("feat/wt-two"));
        assert!(rendered.contains("wt-two"));
    }
}
