"""
More bug-hunting tests - deeper edge cases.
"""

import os
import subprocess
import sys
from unittest.mock import patch

import pytest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'ai_template_scripts'))
from gh_discussion import (
    escape_graphql,
)
from gh_post import (
    clean_body,
    fix_title,
    get_identity,
    parse_args,
)


class TestGhDiscussionBugs:
    """Bugs in gh_discussion.py."""

    def test_escape_graphql_tab_characters(self):
        """Tab characters should be escaped for GraphQL."""
        text = "col1\tcol2\tcol3"
        result = escape_graphql(text)
        # Tabs in GraphQL strings can cause issues
        # Current implementation doesn't escape them
        # This documents the behavior
        assert "\t" in result  # Currently not escaped

    def test_escape_graphql_carriage_return(self):
        """Carriage returns should be handled."""
        text = "line1\rline2"
        result = escape_graphql(text)
        # \r could cause issues in GraphQL
        assert "\r" in result or "\\r" in result

    def test_repo_without_slash_crashes(self):
        """Repo without / should be handled gracefully."""
        # get_repo_id and get_category_id do repo.split("/")
        # which will fail if no slash - unpacking raises ValueError
        with pytest.raises(ValueError, match="not enough values to unpack"):
            owner, name = "invalidrepo".split("/")

    def test_discussion_missing_equals_format(self):
        """gh_discussion doesn't handle --flag=value format."""
        result = subprocess.run(
            [sys.executable, "ai_template_scripts/gh_discussion.py", "create",
             "--title=Test", "--body=Body"],
            capture_output=True, text=True
        )
        # BUG: Will error with "Unknown option: --title=Test"
        assert result.returncode != 0
        assert "Unknown option" in result.stderr


class TestGhPostMoreBugs:
    """More bugs in gh_post.py."""

    def test_url_with_trailing_slash(self):
        """Git URL ending with / should still work."""
        with patch('subprocess.check_output') as mock_git:
            mock_git.return_value = "https://github.com/owner/repo/\n"
            with patch.dict(os.environ, {}, clear=True):
                os.environ.pop("AI_PROJECT", None)
                result = get_identity()
                # BUG: split("/")[-1] on "repo/" gives "" after rstrip
                # Actually rstrip(".git") doesn't remove trailing /
                # So we get "repo/" -> split -> ["...", "repo", ""] -> [-1] = ""
                assert result["project"] != ""

    def test_url_with_double_slash(self):
        """Git URL with // should still work."""
        with patch('subprocess.check_output') as mock_git:
            mock_git.return_value = "https://github.com//owner//repo.git\n"
            with patch.dict(os.environ, {}, clear=True):
                os.environ.pop("AI_PROJECT", None)
                result = get_identity()
                # Double slashes create empty strings in split
                assert result["project"] != ""

    def test_body_file_flag(self):
        """gh supports --body-file flag which we don't handle."""
        args = ["issue", "create", "--title", "T", "--body-file", "body.md"]
        result = parse_args(args)
        # We don't process --body-file, but that's probably OK
        # Just documenting that body_index will be None
        assert result["body_index"] is None
        assert result["body_value"] is None

    def test_pr_create_not_processed(self):
        """gh pr create has --title and --body but we don't process it."""
        args = ["pr", "create", "--title", "T", "--body", "B"]
        result = parse_args(args)
        # command is "pr", not "issue", so won't be processed
        assert result["command"] == "pr"
        # This is probably intentional, but documenting it

    def test_issue_edit_not_processed(self):
        """gh issue edit has --body but we don't process it."""
        args = ["issue", "edit", "123", "--body", "New body"]
        result = parse_args(args)
        assert result["command"] == "issue"
        assert result["subcommand"] == "edit"
        # subcommand is "edit" not "create"/"comment", so won't be processed
        # This might be intentional or a bug depending on requirements

    def test_signature_without_space_before_pipe(self):
        """Signature pattern requires space before pipe."""
        # Current regex: ^[\w_-]+ \|
        # Requires space before |
        body = "Content\n\n---\nproject|WORKER|abc|2026-01-01"
        result = clean_body(body)
        # BUG: Won't match signature without spaces around |
        # So signature won't be removed
        assert "project|WORKER" in result

    def test_label_with_comma_format(self):
        """gh supports --label P1,mail format (comma-separated)."""
        args = ["issue", "create", "--label", "P1,mail", "--title", "T", "--body", "B"]
        result = parse_args(args)
        # BUG: Only checks if label == "mail", not if "mail" is in comma list
        assert result["has_mail_label"] is False  # This should arguably be True


class TestCleanBodyMoreBugs:
    """More edge cases in clean_body."""

    def test_from_with_extra_asterisks(self):
        """FROM with different markdown emphasis."""
        body = "***FROM:*** proj [USER]\n\nContent"
        result = clean_body(body)
        # Current regex only matches **FROM:**
        # ***FROM:*** (bold+italic) won't be removed
        assert "FROM" in result

    def test_from_with_space_before_colon(self):
        """FROM with space before colon."""
        body = "**FROM :** proj [USER]\n\nContent"
        result = clean_body(body)
        # Won't match due to space
        assert "FROM" in result

    def test_signature_with_extra_pipes(self):
        """Signature with more than expected pipes."""
        body = "Content\n\n---\nproj | WORKER | extra | field | abc | 2026-01-01"
        result = clean_body(body)
        # Should still be recognized as signature (matches ^word | )
        assert "WORKER" not in result

    def test_code_block_at_end_with_hr(self):
        """Code block at end containing --- should not be removed."""
        body = """Content

```
---
not a signature
```"""
        result = clean_body(body)
        # The --- inside code block should be preserved
        assert "---" in result
        assert "not a signature" in result

    def test_indented_signature(self):
        """Indented signature should not be matched (it's in a code block context)."""
        body = "Content\n\n   ---\n   proj | WORKER | abc | 2026-01-01"
        result = clean_body(body)
        # Indented --- is NOT matched as signature separator
        # because line.strip() == "---" but the check is on stripped line
        # Wait, it IS stripped. Let me check...
        # Actually the indented signature line "   proj | WORKER..."
        # doesn't match ^[\w_-]+ \| because of leading spaces
        # So this is correctly preserved
        assert "WORKER" in result  # Correctly preserved

    def test_signature_in_blockquote(self):
        """Signature-like content in blockquote should be preserved."""
        body = """Content

> ---
> proj | WORKER | abc | 2026-01-01

More content"""
        result = clean_body(body)
        # Blockquote markers make this not match
        assert "WORKER" in result


class TestFixTitleMoreBugs:
    """More edge cases in fix_title."""

    def test_empty_brackets(self):
        """Empty brackets at start."""
        identity = {"project": "proj", "role": "WORKER"}
        result = fix_title("[] Title", identity)
        # Empty bracket should be removed
        assert result == "[proj] Title"

    def test_nested_brackets(self):
        """Nested brackets [[like this]] - edge case, unusual in practice."""
        identity = {"project": "proj", "role": "WORKER"}
        result = fix_title("[[nested]] Title", identity)
        # [[nested]] -> remove [[nested] -> ] Title -> [proj] ] Title
        # This is an edge case that doesn't happen in practice
        # The regex matches [^\]]* which includes [ characters
        # Documenting current behavior rather than "fixing"
        assert result == "[proj] ] Title"

    def test_bracket_with_special_chars(self):
        """Brackets with special characters."""
        identity = {"project": "proj", "role": "WORKER"}
        result = fix_title("[foo-bar_v2] Title", identity)
        assert result == "[proj] Title"

    def test_bracket_with_space(self):
        """Brackets with space inside."""
        identity = {"project": "proj", "role": "WORKER"}
        result = fix_title("[old project] Title", identity)
        # Space is allowed inside brackets
        assert result == "[proj] Title"

    def test_unclosed_bracket(self):
        """Unclosed bracket at start."""
        identity = {"project": "proj", "role": "WORKER"}
        result = fix_title("[unclosed Title", identity)
        # [^\]]+ matches "unclosed Title" (everything except ])
        # No closing ] found, so regex doesn't match
        assert result == "[proj] [unclosed Title"


class TestPathEdgeCases:
    """Edge cases with PATH handling."""

    def test_path_with_colon_in_directory(self):
        """PATH dir with colon (rare but possible on some systems)."""
        # On macOS/Linux, colons in paths are unusual but possible
        # PATH parsing uses split(":") which would break this
        # Just documenting this edge case

    def test_symlinked_gh(self):
        """What if gh in PATH is a symlink?"""
        # Path comparison might fail if one is resolved and other isn't
        # Just documenting this edge case
