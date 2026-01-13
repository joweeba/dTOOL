"""
Detailed analysis of fix_title behavior - does it produce correct titles?
"""

import os
import sys

import pytest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'ai_template_scripts'))
from gh_post import fix_title


class TestFixTitleActualBehavior:
    """What does fix_title actually produce?"""

    def setup_method(self):
        self.identity = {"project": "myproj", "role": "WORKER"}

    # --- GOOD CASES ---

    def test_plain_title(self):
        """Normal title without brackets."""
        result = fix_title("Fix the login bug", self.identity)
        assert result == "[myproj] Fix the login bug"

    def test_title_with_old_project_prefix(self):
        """Title with existing [project] prefix."""
        result = fix_title("[oldproj] Fix the bug", self.identity)
        assert result == "[myproj] Fix the bug"

    def test_title_with_role_prefix(self):
        """Title with [project][W] prefix."""
        result = fix_title("[oldproj][W] Fix the bug", self.identity)
        assert result == "[myproj] Fix the bug"

    def test_title_with_role_and_number(self):
        """Title with [project][W]123 prefix."""
        result = fix_title("[oldproj][W]123 Fix the bug", self.identity)
        assert result == "[myproj] Fix the bug"

    def test_brackets_in_middle_preserved(self):
        """[URGENT] in middle of title preserved."""
        result = fix_title("[oldproj] [URGENT] Fix the bug", self.identity)
        assert result == "[myproj] [URGENT] Fix the bug"

    def test_brackets_at_end_preserved(self):
        """Brackets at end preserved."""
        result = fix_title("Fix the bug [WIP]", self.identity)
        assert result == "[myproj] Fix the bug [WIP]"

    # --- QUESTIONABLE CASES ---

    def test_many_leading_prefixes(self):
        """Many leading prefixes - project + optional role stripped."""
        result = fix_title("[a][b][c][d] Title", self.identity)
        # [a] stripped as project, [b] stripped as role (single letter)
        # [c][d] preserved (multi-char brackets are tags)
        assert result == "[myproj] [c][d] Title"

    def test_triple_brackets_with_content(self):
        """[project][W][urgent] - role stripped, tag preserved."""
        result = fix_title("[project][W][urgent] Fix bug", self.identity)
        # [project][W] stripped (project + role)
        # [urgent] preserved as a tag
        assert result == "[myproj] [urgent] Fix bug"

    # --- BAD CASES ---

    def test_nested_brackets_munges(self):
        """[[nested]] produces garbage."""
        result = fix_title("[[nested]] Title", self.identity)
        # BUG: This produces "[myproj] ] Title"
        assert result == "[myproj] ] Title"
        # This is WRONG - the stray ] is garbage

    def test_unclosed_bracket_preserved(self):
        """[unclosed is preserved (good? bad?)."""
        result = fix_title("[unclosed Title", self.identity)
        # Regex requires closing ], so this doesn't match
        assert result == "[myproj] [unclosed Title"
        # This is correct - we add prefix before the unclosed bracket

    def test_just_closing_bracket(self):
        """] at start is preserved."""
        result = fix_title("] Some title", self.identity)
        # Doesn't match ^\[, so preserved
        assert result == "[myproj] ] Some title"
        # Acceptable - we can't fix malformed input

    def test_bracket_with_newline_inside(self):
        """Bracket containing newline."""
        result = fix_title("[line1\nline2] Title", self.identity)
        # [^\]]* matches newlines
        assert result == "[myproj] Title"
        # This is probably correct

    def test_empty_title_after_stripping(self):
        """Title with only brackets - project + role stripped."""
        result = fix_title("[a][b][c]", self.identity)
        # [a] + [b] stripped (project + single-letter role)
        # [c] preserved
        assert result == "[myproj] [c]"

    def test_title_is_just_spaces_after_stripping(self):
        """Title that becomes just spaces."""
        result = fix_title("[a]   ", self.identity)
        # Strips [a], then \s* strips spaces
        assert result == "[myproj] "
        # Empty title

    def test_no_space_after_bracket(self):
        """[proj]Title without space."""
        result = fix_title("[proj]Title", self.identity)
        # \s* is zero or more, so "Title" preserved
        assert result == "[myproj] Title"
        # Good - normalized

    def test_multiple_spaces_after_bracket(self):
        """[proj]   Title with multiple spaces."""
        result = fix_title("[proj]   Title", self.identity)
        # \s* consumes all spaces
        assert result == "[myproj] Title"
        # Good - normalized

    # --- WHAT ABOUT ERRORS? ---

    def test_never_raises_error(self):
        """fix_title never raises - is this good?"""
        # Various problematic inputs
        problematic = [
            "",
            "   ",
            "[[[[",
            "]]]]",
            "\n\n\n",
            None,  # This WILL raise
        ]
        for title in problematic[:-1]:  # Skip None
            result = fix_title(title, self.identity)
            assert isinstance(result, str)

    def test_none_input_raises(self):
        """None input raises TypeError."""
        with pytest.raises(TypeError):
            fix_title(None, self.identity)


class TestFixTitleShouldWarn:
    """Cases where fix_title should maybe warn or error."""

    def setup_method(self):
        self.identity = {"project": "proj", "role": "WORKER"}

    def test_produces_garbage_output(self):
        """Known case that produces garbage."""
        result = fix_title("[[nested]] Title", self.identity)
        # Contains stray ] - [proj] ] Title has "] " in it after [proj]
        assert "proj] ]" in result  # This is garbage

    def test_brackets_only_title(self):
        """Title that's only brackets - first stripped, rest preserved."""
        result = fix_title("[][][]", self.identity)
        # [] stripped, [][] preserved
        assert result == "[proj] [][]"

    def test_title_looks_like_command(self):
        """Title that looks like shell command."""
        result = fix_title("rm -rf /", self.identity)
        # Just passes through
        assert "rm -rf /" in result
        # Not fix_title's job to validate, but interesting


class TestFixTitleVsExpectedAIBehavior:
    """What titles do AIs actually produce?"""

    def setup_method(self):
        self.identity = {"project": "proj", "role": "WORKER"}

    def test_ai_typically_produces(self):
        """AIs typically produce titles like these."""
        typical = [
            "Fix authentication bug in login flow",
            "[proj] Add user registration feature",
            "[proj][W] Refactor database queries",
            "[proj][W]42 Update API endpoints",
            "Add tests for payment module",
        ]
        for title in typical:
            result = fix_title(title, self.identity)
            # Should produce clean [proj] prefixed title
            assert result.startswith("[proj] ")
            # Should not have stray characters
            assert "] ]" not in result
            assert "[[" not in result

    def test_ai_edge_cases(self):
        """Edge cases AIs might produce."""
        edge_cases = [
            ("[other_proj] Task",       "[proj] Task"),           # Different project
            ("[proj][M] Task",          "[proj] Task"),           # Manager prefix
            ("[proj][W]123 Task",       "[proj] Task"),           # With iteration
            ("Task [Part 1]",           "[proj] Task [Part 1]"),  # Suffix preserved
            ("[URGENT][proj] Task",     "[proj] [proj] Task"),    # Tag before project - unusual input
        ]
        for input_title, expected in edge_cases:
            result = fix_title(input_title, self.identity)
            assert result == expected, f"Input: {input_title}"
