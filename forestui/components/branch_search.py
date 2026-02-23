"""Fuzzy branch search widget with dropdown results."""

from __future__ import annotations

import contextlib
import re

from rich.text import Text
from textual.app import ComposeResult
from textual.containers import Vertical
from textual.message import Message
from textual.suggester import Suggester
from textual.widgets import Input, Label, OptionList
from textual.widgets.option_list import Option

MAX_DROPDOWN_RESULTS = 50

# Common remote prefixes to strip when matching
_REMOTE_PREFIXES = ("origin", "upstream", "fork")


def _levenshtein_distance(s1: str, s2: str) -> int:
    """Calculate Levenshtein distance between two strings."""
    if len(s1) < len(s2):
        return _levenshtein_distance(s2, s1)
    if len(s2) == 0:
        return len(s1)

    prev_row = list(range(len(s2) + 1))
    for _i, c1 in enumerate(s1):
        curr_row = [prev_row[0] + 1]
        for j, c2 in enumerate(s2):
            insertions = prev_row[j + 1] + 1
            deletions = curr_row[j] + 1
            substitutions = prev_row[j] + (c1 != c2)
            curr_row.append(min(insertions, deletions, substitutions))
        prev_row = curr_row
    return prev_row[-1]


def _strip_remote_prefix(branch: str) -> str:
    """Strip remote prefix (e.g., origin/) from a branch name."""
    if "/" in branch:
        prefix, rest = branch.split("/", 1)
        if prefix in _REMOTE_PREFIXES:
            return rest
    return branch


def _match_score(query: str, branch: str) -> float | None:
    """Calculate match score for a branch against a query.

    Lower score = better match. Returns None for no match.

    Scoring tiers:
        0.0 - Exact match
        0.5 - Exact match on local name (without remote prefix)
        1.0 - Prefix match on full name
        1.5 - Prefix match on local name
        2.0 - Substring match at word boundary
        3.0 - Substring match anywhere
        4.0+ - Fuzzy match (Levenshtein) on path segments
    """
    q = query.lower()
    b = branch.lower()

    if not q:
        return 0.0

    # Exact match
    if q == b:
        return 0.0

    local = _strip_remote_prefix(b)

    # Exact match on local part
    if q == local:
        return 0.5

    # Prefix match
    if b.startswith(q):
        return 1.0
    if local.startswith(q):
        return 1.5

    # Substring match
    for target, base_score in [(b, 2.0), (local, 2.0)]:
        idx = target.find(q)
        if idx >= 0:
            # Bonus for word boundary match
            if idx == 0 or target[idx - 1] in "/-_.":
                return base_score
            return 3.0

    # Fuzzy matching on path segments (only for queries >= 2 chars)
    if len(q) >= 2:
        segments = re.split(r"[/\-_.]", b)
        best_score: float | None = None
        threshold = max(1, (len(q) + 2) // 3)

        for seg in segments:
            if not seg:
                continue
            seg_lower = seg.lower()

            # Levenshtein on full segment
            dist = _levenshtein_distance(q, seg_lower)
            if dist <= threshold:
                score = 4.0 + dist * 0.1
                if best_score is None or score < best_score:
                    best_score = score

            # Levenshtein on segment prefix (for partial typed matches)
            if len(seg_lower) > len(q):
                prefix_dist = _levenshtein_distance(q, seg_lower[: len(q)])
                if prefix_dist <= threshold:
                    score = 4.5 + prefix_dist * 0.1
                    if best_score is None or score < best_score:
                        best_score = score

        if best_score is not None:
            return best_score

    return None


def fuzzy_match_branches(
    query: str, branches: list[str], max_results: int = MAX_DROPDOWN_RESULTS
) -> list[tuple[str, float]]:
    """Match branches against query with fuzzy matching.

    Returns list of (branch_name, score) tuples, sorted by score (lower = better).
    """
    if not query.strip():
        return [(b, 0.0) for b in branches[:max_results]]

    results: list[tuple[str, float]] = []
    for branch in branches:
        score = _match_score(query, branch)
        if score is not None:
            results.append((branch, score))

    results.sort(key=lambda x: (x[1], x[0].lower()))
    return results[:max_results]


def _highlight_match(query: str, branch: str) -> Text:
    """Create a Rich Text with the matching portion highlighted."""
    if not query:
        return Text(branch)

    q_lower = query.lower()
    b_lower = branch.lower()

    idx = b_lower.find(q_lower)
    if idx >= 0:
        text = Text()
        if idx > 0:
            text.append(branch[:idx])
        text.append(branch[idx : idx + len(query)], style="bold reverse")
        remaining = branch[idx + len(query) :]
        if remaining:
            text.append(remaining)
        return text

    # No substring match found (fuzzy match) - show branch as-is
    return Text(branch)


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
        placeholder: str = "Start typing to search branches...",
        value: str = "",
        widget_id: str | None = None,
    ) -> None:
        super().__init__(id=widget_id)
        self._branches = branches
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
        matches = fuzzy_match_branches(query, self._branches)

        option_list = self.query_one(OptionList)
        option_list.clear_options()

        for branch, _score in matches:
            display = _highlight_match(query, branch)
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

    def update_branches(self, branches: list[str]) -> None:
        """Update the branch list (e.g., after a fetch)."""
        self._branches = branches
        current_value = self.selected_value
        self._update_results(current_value)


class FuzzyBranchSuggester(Suggester):
    """A Textual Suggester that uses fuzzy matching for inline suggestions."""

    def __init__(self, branches: list[str]) -> None:
        super().__init__(use_cache=False, case_sensitive=False)
        self._branches = branches

    async def get_suggestion(self, value: str) -> str | None:
        """Return the best fuzzy match for inline suggestion."""
        if not value:
            return None
        matches = fuzzy_match_branches(value, self._branches, max_results=1)
        if matches:
            return matches[0][0]
        return None

    def update_branches(self, branches: list[str]) -> None:
        """Update the branch list."""
        self._branches = branches
