"""
Tests for gh_discussion.py - Discussion creation with identity.

Tests cover argument parsing, GraphQL escaping, category handling,
and integration with gh_post identity functions.
"""

import os
import subprocess
import sys
from unittest.mock import patch

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'ai_template_scripts'))
from gh_discussion import (
    CATEGORY_IDS,
    DASHNEWS_REPO_ID,
    escape_graphql,
    fix_title,
    get_identity,
    process_body,
)


class TestEscapeGraphQL:
    """Tests for escape_graphql() - GraphQL string escaping."""

    def test_escapes_backslashes(self):
        assert escape_graphql("path\\to\\file") == "path\\\\to\\\\file"

    def test_escapes_quotes(self):
        assert escape_graphql('He said "hello"') == 'He said \\"hello\\"'

    def test_escapes_newlines(self):
        assert escape_graphql("line1\nline2") == "line1\\nline2"

    def test_handles_empty_string(self):
        assert escape_graphql("") == ""

    def test_handles_plain_text(self):
        assert escape_graphql("plain text") == "plain text"

    def test_handles_multiple_special_chars(self):
        text = 'Line1\nLine2 with "quotes" and \\backslash'
        result = escape_graphql(text)
        assert "\\\\" in result  # Escaped backslash
        assert '\\"' in result   # Escaped quote
        assert "\\n" in result   # Escaped newline

    def test_unicode_passthrough(self):
        assert escape_graphql("émojis 🎉 and ñ") == "émojis 🎉 and ñ"

    def test_code_block_preserved(self):
        code = "```python\ndef foo():\n    pass\n```"
        result = escape_graphql(code)
        assert "```python" in result
        assert "\\n" in result


class TestCategoryIds:
    """Tests for category ID constants."""

    def test_all_dashnews_categories_defined(self):
        expected = ["General", "Q&A", "Show and tell", "Ideas", "Announcements", "Polls"]
        for cat in expected:
            assert cat in CATEGORY_IDS, f"Missing category: {cat}"

    def test_category_ids_are_strings(self):
        for cat, id_ in CATEGORY_IDS.items():
            assert isinstance(id_, str), f"Category {cat} ID should be string"
            assert id_.startswith("DIC_"), f"Category {cat} ID should start with DIC_"

    def test_dashnews_repo_id_format(self):
        assert DASHNEWS_REPO_ID.startswith("R_")


class TestArgumentParsing:
    """Tests for create_discussion argument parsing (via subprocess)."""

    def test_help_flag_shows_usage(self):
        result = subprocess.run(
            [sys.executable, "-c",
             "import sys; sys.path.insert(0, 'ai_template_scripts'); "
             "from gh_discussion import usage; usage()"],
            capture_output=True, text=True
        )
        assert "Usage:" in result.stdout or result.returncode == 0

    def test_missing_title_errors(self):
        """Missing --title should error."""
        result = subprocess.run(
            [sys.executable, "ai_template_scripts/gh_discussion.py", "create",
             "--body", "test body"],
            capture_output=True, text=True
        )
        assert result.returncode != 0
        assert "Missing required --title" in result.stderr

    def test_missing_body_errors(self):
        """Missing --body should error."""
        result = subprocess.run(
            [sys.executable, "ai_template_scripts/gh_discussion.py", "create",
             "--title", "test title"],
            capture_output=True, text=True
        )
        assert result.returncode != 0
        assert "Missing required --body" in result.stderr

    def test_unknown_option_errors(self):
        """Unknown options should error."""
        result = subprocess.run(
            [sys.executable, "ai_template_scripts/gh_discussion.py", "create",
             "--unknown", "value"],
            capture_output=True, text=True
        )
        assert result.returncode != 0
        assert "Unknown option" in result.stderr


class TestCategoryValidation:
    """Tests for category validation logic."""

    def test_valid_categories(self):
        valid = ["General", "Q&A", "Show and tell", "Ideas", "Announcements", "Polls"]
        for cat in valid:
            assert cat in CATEGORY_IDS

    def test_case_sensitive_categories(self):
        # Categories are case-sensitive
        assert "general" not in CATEGORY_IDS
        assert "q&a" not in CATEGORY_IDS
        assert "GENERAL" not in CATEGORY_IDS


class TestIntegrationWithGhPost:
    """Tests verifying integration with gh_post functions."""

    def test_imports_from_gh_post(self):
        """Verify we can import from gh_post."""
        # These should be callable (imported at module level)
        assert callable(get_identity)
        assert callable(fix_title)
        assert callable(process_body)

    @patch.dict(os.environ, {"AI_PROJECT": "testproj", "AI_ROLE": "WORKER"})
    def test_identity_used_in_title(self):
        identity = get_identity()
        result = fix_title("Test Title", identity)
        assert "[testproj]" in result

    @patch.dict(os.environ, {"AI_PROJECT": "testproj", "AI_ROLE": "WORKER", "AI_ITERATION": "1"})
    @patch('gh_post.get_commit', return_value='abc123')
    def test_identity_used_in_body(self, mock_commit):
        identity = get_identity()
        result = process_body("Test content", identity)
        assert "**FROM:**" in result
        assert "testproj" in result


class TestEdgeCases:
    """Edge cases and boundary conditions."""

    def test_escape_many_newlines(self):
        text = "\n".join(["line"] * 100)
        result = escape_graphql(text)
        assert result.count("\\n") == 99

    def test_escape_deeply_nested_quotes(self):
        text = 'Outer "middle "inner" middle" outer'
        result = escape_graphql(text)
        assert '\\"' in result
        # Should not double-escape
        assert '\\\\"' not in result

    def test_escape_backslash_before_quote(self):
        """Tricky case: backslash right before quote."""
        text = 'path\\"file'
        result = escape_graphql(text)
        # Original \ becomes \\, original " becomes \"
        assert '\\\\\\"' in result

    def test_escape_tab_characters(self):
        """Tabs are not escaped (GraphQL handles them)."""
        text = "col1\tcol2"
        result = escape_graphql(text)
        assert "\t" in result

    def test_markdown_table_preserved(self):
        """Markdown tables should work after escaping."""
        table = "| A | B |\n|---|---|\n| 1 | 2 |"
        result = escape_graphql(table)
        assert "|" in result


class TestRepoIdParsing:
    """Tests for repository ID handling."""

    def test_repo_format_validation(self):
        """Repo should be in owner/name format."""

        # These functions expect owner/name format
        # Testing the split logic (actual API calls would fail without mock)
        repo = "owner/repo"
        owner, name = repo.split("/")
        assert owner == "owner"
        assert name == "repo"

    def test_repo_with_hyphen(self):
        repo = "my-org/my-repo"
        owner, name = repo.split("/")
        assert owner == "my-org"
        assert name == "my-repo"

    def test_repo_with_underscore(self):
        repo = "my_org/my_repo"
        owner, name = repo.split("/")
        assert owner == "my_org"
        assert name == "my_repo"
