"""Fuzzy branch search widget with dropdown results."""

from __future__ import annotations

import contextlib

from textual.app import ComposeResult
from textual.containers import Vertical
from textual.message import Message
from textual.suggester import Suggester
from textual.widgets import Input, Label, OptionList
from textual.widgets.option_list import Option

from forestui.utils import fuzzy_match_branches, highlight_match


class BranchSearchInput(Vertical):
    """Input widget with fuzzy branch search and a visible dropdown of results."""

    DEFAULT_CSS = """
    BranchSearchInput {
        height: auto;
    }

    BranchSearchInput OptionList {
        max-height: 10;
        height: auto;
        margin-top: 0;
        background: $surface;
    }

    BranchSearchInput .match-count {
        color: $text-muted;
        height: 1;
        padding: 0 1;
    }
    """

    class Changed(Message):
        """Emitted when the input value changes."""

        def __init__(self, value: str, branch_search: BranchSearchInput) -> None:
            self.value = value
            self.branch_search = branch_search
            super().__init__()

    class BranchSelected(Message):
        """Emitted when a branch is explicitly selected from the dropdown."""

        def __init__(self, branch: str, branch_search: BranchSearchInput) -> None:
            self.branch = branch
            self.branch_search = branch_search
            super().__init__()

    def __init__(
        self,
        branches: list[str],
        *,
        remotes: list[str] | None = None,
        placeholder: str = "Start typing to search branches...",
        value: str = "",
        widget_id: str | None = None,
    ) -> None:
        super().__init__(id=widget_id)
        self._branches = branches
        self._remotes = remotes or []
        self._placeholder = placeholder
        self._initial_value = value
        self._selected_branch: str = value
        self._updating_from_selection = False

    @property
    def selected_value(self) -> str:
        """Get the current input value."""
        try:
            return self.query_one(Input).value
        except Exception:
            return self._selected_branch

    def set_value(self, val: str) -> None:
        """Set the input value programmatically."""
        self._updating_from_selection = True
        with contextlib.suppress(Exception):
            self.query_one(Input).value = val
        self._selected_branch = val
        self._updating_from_selection = False

    def compose(self) -> ComposeResult:
        """Compose the widget."""
        yield Input(placeholder=self._placeholder, value=self._initial_value)
        yield Label("", classes="match-count")
        yield OptionList()

    def on_mount(self) -> None:
        """Initialize the option list."""
        self._update_results(self._initial_value)

    def on_input_changed(self, event: Input.Changed) -> None:
        """Handle input changes - filter branches."""
        event.stop()  # Prevent bubbling to parent

        if self._updating_from_selection:
            return

        self._update_results(event.value)
        self._selected_branch = event.value
        self.post_message(self.Changed(event.value, self))

    def on_option_list_option_selected(
        self,
        event: OptionList.OptionSelected,
    ) -> None:
        """Handle branch selection from dropdown."""
        event.stop()

        option_id = event.option_id
        if option_id is None:
            return

        branch = str(option_id)
        self._updating_from_selection = True
        self.query_one(Input).value = branch
        self._updating_from_selection = False
        self._selected_branch = branch
        self._update_results(branch)
        self.post_message(self.Changed(branch, self))
        self.post_message(self.BranchSelected(branch, self))
        # Return focus to input
        self.query_one(Input).focus()

    def _update_results(self, query: str) -> None:
        """Update the dropdown with matching branches."""
        matches = fuzzy_match_branches(query, self._branches, remotes=self._remotes)

        option_list = self.query_one(OptionList)
        option_list.clear_options()

        for branch, _score in matches:
            display = highlight_match(query, branch)
            option_list.add_option(Option(display, id=branch))

        # Update match count
        count_label = self.query_one(".match-count", Label)
        total = len(self._branches)
        shown = len(matches)
        if not query.strip():
            if shown < total:
                count_label.update(f"{shown} of {total} branches")
            else:
                count_label.update(f"{total} branches")
        elif shown == 0:
            count_label.update("No matches")
        else:
            count_label.update(f"{shown} match{'es' if shown != 1 else ''}")

    def update_branches(
        self, branches: list[str], remotes: list[str] | None = None
    ) -> None:
        """Update the branch list (e.g., after a fetch)."""
        self._branches = branches
        if remotes is not None:
            self._remotes = remotes
        current_value = self.selected_value
        self._update_results(current_value)


class FuzzyBranchSuggester(Suggester):
    """A Textual Suggester that uses fuzzy matching for inline suggestions."""

    def __init__(self, branches: list[str], remotes: list[str] | None = None) -> None:
        super().__init__(use_cache=False, case_sensitive=False)
        self._branches = branches
        self._remotes = remotes or []

    async def get_suggestion(self, value: str) -> str | None:
        """Return the best fuzzy match for inline suggestion."""
        if not value:
            return None
        matches = fuzzy_match_branches(
            value, self._branches, remotes=self._remotes, max_results=1
        )
        if matches:
            return matches[0][0]
        return None
