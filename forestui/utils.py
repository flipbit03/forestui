"""Utility functions for fuzzy branch matching."""

from __future__ import annotations

import re

from rich.text import Text

MAX_DROPDOWN_RESULTS = 50


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


def strip_remote_prefix(branch: str, remotes: list[str]) -> str:
    """Strip remote prefix (e.g., origin/) from a branch name.

    Uses the actual list of git remotes rather than hardcoded names.
    """
    if "/" in branch:
        prefix, rest = branch.split("/", 1)
        if prefix in remotes:
            return rest
    return branch


def _match_score(query: str, branch: str, remotes: list[str]) -> float | None:
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

    local = strip_remote_prefix(b, remotes)

    # Exact match on local part
    if q == local:
        return 0.5

    # Prefix match
    if b.startswith(q):
        return 1.0
    if local.startswith(q):
        return 1.5

    # Substring match
    for target in [b, local]:
        idx = target.find(q)
        if idx >= 0:
            # Bonus for word boundary match
            if idx == 0 or target[idx - 1] in "/-_.":
                return 2.0
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
    query: str,
    branches: list[str],
    remotes: list[str] | None = None,
    max_results: int = MAX_DROPDOWN_RESULTS,
) -> list[tuple[str, float]]:
    """Match branches against query with fuzzy matching.

    Args:
        query: Search string typed by the user.
        branches: Full list of branch names (local + remote).
        remotes: List of git remote names (from ``git remote``).
            Used to correctly strip remote prefixes during matching.
        max_results: Maximum number of results to return.

    Returns:
        List of (branch_name, score) tuples, sorted by score (lower = better).
    """
    effective_remotes = remotes if remotes is not None else []

    if not query.strip():
        return [(b, 0.0) for b in branches[:max_results]]

    results: list[tuple[str, float]] = []
    for branch in branches:
        score = _match_score(query, branch, effective_remotes)
        if score is not None:
            results.append((branch, score))

    results.sort(key=lambda x: (x[1], x[0].lower()))
    return results[:max_results]


def highlight_match(query: str, branch: str) -> Text:
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
