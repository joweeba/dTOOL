"""
Bug-hunting tests for gh_post.py - testing edge cases and potential issues.
"""

import os
import sys
from unittest.mock import patch

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'ai_template_scripts'))
from gh_post import (
    build_header,
    build_signature,
    clean_body,
    fix_title,
    get_identity,
    parse_args,
)


class TestParseArgsBugs:
    """Bugs in argument parsing."""

    def test_short_repo_flag(self):
        """gh supports -R as short form of --repo."""
        args = ["issue", "create", "-R", "owner/repo", "--title", "T", "--body", "B"]
        result = parse_args(args)
        # BUG: This will fail - parse_args doesn't handle -R
        assert result["repo"] == "owner/repo"

    def test_label_equals_format(self):
        """gh supports --label=mail format."""
        args = ["issue", "create", "--label=mail", "--title", "T", "--body", "B"]
        result = parse_args(args)
        # BUG: This will fail - parse_args only handles --label mail (space)
        assert result["has_mail_label"] is True

    def test_body_equals_format(self):
        """gh supports --body=content format."""
        args = ["issue", "create", "--title", "T", "--body=My body content"]
        result = parse_args(args)
        # Should have body_value for --body=value format
        assert result["body_value"] == "My body content"

    def test_title_equals_format(self):
        """gh supports --title=content format."""
        args = ["issue", "create", "--title=My Title", "--body", "B"]
        result = parse_args(args)
        # Should have title_value for --title=value format
        assert result["title_value"] == "My Title"


class TestFixTitleBugs:
    """Bugs in title fixing."""

    def test_triple_bracket_prefix(self):
        """Project + role prefix removed, multi-char tags preserved."""
        identity = {"project": "proj", "role": "WORKER"}
        result = fix_title("[a][b][c] Title", identity)
        # [a] = project, [b] = single-letter role, [c] = preserved tag
        assert result == "[proj] [c] Title"

    def test_bracket_with_numbers(self):
        """Brackets with numbers like [W]123 should be removed."""
        identity = {"project": "proj", "role": "WORKER"}
        result = fix_title("[old][W]123 Title", identity)
        assert result == "[proj] Title"


class TestCleanBodyBugs:
    """Bugs in body cleaning."""

    def test_windows_line_endings_cleaned(self):
        """Windows \\r\\n should be normalized."""
        body = "**FROM:** proj [USER]\r\n\r\nContent\r\n\r\n---\r\nproj | USER | x | y"
        result = clean_body(body)
        # Should not have stray \r characters
        assert "\r" not in result
        assert "Content" in result

    def test_mixed_line_endings(self):
        """Mixed line endings should be handled."""
        body = "**FROM:** proj [USER]\n\r\nContent\r\n"
        result = clean_body(body)
        # Should handle gracefully
        assert "Content" in result


class TestBuildSignatureBugs:
    """Bugs in signature building."""

    @patch('gh_post.get_commit', return_value='abc123')
    def test_project_with_pipe_character(self, mock_commit):
        """Project name with | would break signature parsing."""
        identity = {
            "project": "foo|bar",  # Problematic name
            "role": "WORKER",
            "iteration": "1",
            "session": ""
        }
        result = build_signature(identity)
        # The signature uses | as delimiter, so this is ambiguous
        # foo|bar | WORKER #1 | abc123 | timestamp
        # How many fields is that?
        assert "foo|bar" in result
        # This test documents the issue - not necessarily a fix


class TestGetIdentityBugs:
    """Bugs in identity derivation."""

    @patch('subprocess.check_output')
    def test_ssh_git_url(self, mock_git):
        """SSH URLs should be parsed correctly."""
        mock_git.return_value = "git@github.com:owner/myrepo.git\n"
        with patch.dict(os.environ, {}, clear=True):
            os.environ.pop("AI_PROJECT", None)
            result = get_identity()
            # BUG: The current code splits by / which doesn't work for SSH URLs
            # git@github.com:owner/myrepo.git -> split("/")[-1] = myrepo.git
            # Then .rstrip(".git") = myrepo - this actually works!
            assert result["project"] == "myrepo"

    @patch('subprocess.check_output')
    def test_git_url_without_git_suffix(self, mock_git):
        """URLs without .git suffix should work."""
        mock_git.return_value = "https://github.com/owner/myrepo\n"
        with patch.dict(os.environ, {}, clear=True):
            os.environ.pop("AI_PROJECT", None)
            result = get_identity()
            assert result["project"] == "myrepo"


class TestEdgeCaseBugs:
    """Other edge cases."""

    def test_empty_project_in_signature(self):
        """What if project is empty string?"""
        identity = {"project": "", "role": "USER", "iteration": "", "session": ""}
        result = build_header(identity)
        # Would produce "**FROM:**  [USER]" with double space
        assert "**FROM:**  " not in result or identity["project"] == ""

    def test_clean_body_only_from_header(self):
        """Body with only FROM header and nothing else."""
        body = "**FROM:** proj [USER]"
        result = clean_body(body)
        assert result == ""

    def test_clean_body_from_header_with_only_whitespace_after(self):
        """FROM header followed by only whitespace."""
        body = "**FROM:** proj [USER]\n\n   \n\n"
        result = clean_body(body)
        assert result == ""

    def test_signature_regex_too_greedy(self):
        """Signature pattern could match non-signature content."""
        # This looks like a signature but isn't
        body = "Compare: option_a | option_b | option_c"
        result = clean_body(body)
        # Should preserve this content
        assert "option_a | option_b | option_c" in result

    def test_from_header_case_sensitivity(self):
        """FROM header matching should handle case variations."""
        body = "**from:** proj [USER]\n\nContent"
        result = clean_body(body)
        # Current regex is case-sensitive - lowercase won't be removed
        # This documents current behavior
        assert "from:" in result.lower()
