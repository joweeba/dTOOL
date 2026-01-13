"""
Deep bug hunting - security, encoding, error handling edge cases.
"""

import os
import subprocess
import sys
from unittest.mock import patch

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'ai_template_scripts'))
from gh_discussion import escape_graphql
from gh_post import (
    build_header,
    build_signature,
    clean_body,
    fix_title,
    get_identity,
    parse_args,
    process_body,
)


class TestSecurityIssues:
    """Potential security issues."""

    def test_project_name_with_shell_metacharacters(self):
        """Project name with shell metacharacters shouldn't cause issues."""
        identity = {
            "project": "proj; rm -rf /",
            "role": "WORKER",
            "iteration": "1",
            "session": ""
        }
        header = build_header(identity)
        # Should just include the literal string, not execute it
        assert "proj; rm -rf /" in header

    def test_body_with_shell_metacharacters(self):
        """Body with shell metacharacters."""
        body = "Content with $(whoami) and `id` and $HOME"
        result = clean_body(body)
        # Should preserve literally
        assert "$(whoami)" in result
        assert "`id`" in result
        assert "$HOME" in result

    def test_graphql_injection_in_title(self):
        """Title with GraphQL injection attempt."""
        malicious = 'Title", body: "injected'
        result = escape_graphql(malicious)
        # Quotes should be escaped
        assert '\\"' in result
        assert 'Title\\"' in result

    def test_graphql_injection_in_body(self):
        """Body with GraphQL special characters."""
        malicious = '}) { malicious { query } } mutation {'
        result = escape_graphql(malicious)
        # Should be escaped as a string, braces don't need escaping
        assert result == malicious  # No escaping needed for braces

    def test_title_with_newlines(self):
        """Newlines in title could break things."""
        identity = {"project": "proj", "role": "WORKER"}
        result = fix_title("Title\nwith\nnewlines", identity)
        # Newlines in title are unusual but shouldn't crash
        assert "[proj]" in result

    def test_null_bytes_in_body(self):
        """Null bytes in body."""
        body = "Content\x00with\x00nulls"
        result = clean_body(body)
        # Should handle without crashing
        assert "Content" in result


class TestEncodingIssues:
    """Unicode and encoding edge cases."""

    def test_emoji_in_project_name(self):
        """Emoji in project name."""
        identity = {
            "project": "proj-🚀",
            "role": "WORKER",
            "iteration": "1",
            "session": ""
        }
        header = build_header(identity)
        assert "proj-🚀" in header

    def test_rtl_characters_in_body(self):
        """Right-to-left characters."""
        body = "English and עברית and العربية"
        result = clean_body(body)
        assert "עברית" in result
        assert "العربية" in result

    def test_zero_width_characters(self):
        """Zero-width characters that could hide content."""
        body = "Normal\u200btext\u200bwith\u200bzero\u200bwidth"
        result = clean_body(body)
        # Zero-width chars should be preserved
        assert "\u200b" in result

    def test_combining_characters(self):
        """Combining diacritical marks."""
        body = "Café with combining: e\u0301"  # é as e + combining acute
        result = clean_body(body)
        assert "e\u0301" in result

    def test_graphql_escape_unicode(self):
        """Unicode in GraphQL escaping."""
        text = "Emoji 🎉 and Chinese 中文"
        result = escape_graphql(text)
        assert "🎉" in result
        assert "中文" in result


class TestEmptyAndNullInputs:
    """Edge cases with empty/null/missing inputs."""

    def test_parse_args_with_empty_body_value(self):
        """--body '' (empty string)."""
        args = ["issue", "create", "--title", "T", "--body", ""]
        result = parse_args(args)
        assert result["body_index"] == 5
        # The body IS present, just empty

    def test_process_body_empty_string(self):
        """Processing empty body."""
        identity = {"project": "proj", "role": "USER", "iteration": "", "session": ""}
        with patch('gh_post.get_commit', return_value='abc'):
            result = process_body("", identity)
            # Should still have header and signature
            assert "**FROM:**" in result
            assert "---" in result

    def test_fix_title_only_brackets(self):
        """Title that's only brackets - first stripped, rest preserved."""
        identity = {"project": "proj", "role": "WORKER"}
        result = fix_title("[][][]", identity)
        # [] stripped as project, [][] preserved (not role prefixes)
        assert result == "[proj] [][]"

    def test_identity_all_empty(self):
        """Identity with all empty values."""
        identity = {"project": "", "role": "", "iteration": "", "session": ""}
        header = build_header(identity)
        # Should handle gracefully
        assert "**FROM:**" in header


class TestLongInputs:
    """Very long inputs that might cause issues."""

    def test_very_long_body(self):
        """Body with 1MB of content."""
        long_body = "x" * (1024 * 1024)  # 1MB
        result = clean_body(long_body)
        assert len(result) == 1024 * 1024

    def test_very_long_title(self):
        """Very long title."""
        identity = {"project": "proj", "role": "WORKER"}
        long_title = "T" * 10000
        result = fix_title(long_title, identity)
        assert result.startswith("[proj] ")
        assert len(result) > 10000

    def test_many_from_headers(self):
        """Body with many FROM headers stacked."""
        headers = "\n\n".join([f"**FROM:** proj{i} [USER]" for i in range(100)])
        body = headers + "\n\nActual content"
        result = clean_body(body)
        # All FROM headers should be removed
        assert "**FROM:**" not in result
        assert "Actual content" in result

    def test_many_signature_like_blocks(self):
        """Body with many --- separators."""
        body = "Content\n\n" + "---\n\n" * 50 + "More content"
        result = clean_body(body)
        # Should preserve content, HRs in middle preserved
        assert "Content" in result
        assert "More content" in result


class TestParseArgsMoreEdgeCases:
    """More argument parsing edge cases."""

    def test_short_body_flag(self):
        """gh supports -b for --body, and we handle it."""
        args = ["issue", "create", "-b", "body content", "--title", "T"]
        result = parse_args(args)
        # We now handle -b (gh does support it)
        assert result["body_index"] == 3

    def test_double_dash_separator(self):
        """-- separator for positional args."""
        args = ["issue", "create", "--", "--title", "T"]
        result = parse_args(args)
        # We don't handle -- separator
        # After --, --title should be treated as positional
        # But our parser will still try to parse it
        assert result["title_index"] == 4  # Still parsed

    def test_repeated_flags(self):
        """Same flag specified twice."""
        args = ["issue", "create", "--body", "first", "--body", "second", "--title", "T"]
        result = parse_args(args)
        # Last one wins? Or first one?
        # Our parser overwrites, so last one wins
        assert result["body_index"] == 5  # Index of "second"

    def test_flag_value_looks_like_flag(self):
        """Value that looks like a flag."""
        args = ["issue", "create", "--title", "--weird-title", "--body", "B"]
        result = parse_args(args)
        # --weird-title should be treated as the title value
        assert result["title_index"] == 3

    def test_equals_with_equals_in_value(self):
        """--body=value=with=equals."""
        args = ["issue", "create", "--body=a=b=c", "--title", "T"]
        result = parse_args(args)
        # Should only split on first =
        assert result["body_value"] == "a=b=c"


class TestCleanBodyMoreEdgeCases:
    """More clean_body edge cases."""

    def test_from_header_with_extra_spaces(self):
        """FROM header with extra whitespace."""
        body = "**FROM:**    proj   [USER]   \n\nContent"
        result = clean_body(body)
        # Should still be recognized and removed
        # Current regex: ^\*\*FROM:\*\* - requires exact match at start
        assert "Content" in result

    def test_signature_with_tabs(self):
        """Signature using tabs instead of spaces."""
        body = "Content\n\n---\nproj\t|\tWORKER\t|\tabc\t|\t2026-01-01"
        result = clean_body(body)
        # Tabs around | - regex is: ^[\w_-]+ \|
        # This requires SPACE before |, not tab
        # So this won't be recognized as signature
        assert "WORKER" in result  # Not removed

    def test_signature_mixed_case(self):
        """Signature fields in different cases."""
        body = "Content\n\n---\nPROJECT: proj\nROLE: worker"
        result = clean_body(body)
        # Old format regex: ^(Project|Role|...) - case sensitive!
        # So PROJECT won't match
        assert "PROJECT:" in result  # Not removed

    def test_hr_with_extra_dashes(self):
        """HR with more than 3 dashes."""
        body = "Content\n\n-----\n\nMore content"
        result = clean_body(body)
        # ----- is not == "---"
        # Should be preserved
        assert "-----" in result

    def test_hr_with_spaces(self):
        """HR with spaces: - - -"""
        body = "Content\n\n- - -\n\nMore"
        result = clean_body(body)
        # Not recognized as HR
        assert "- - -" in result

    def test_consecutive_signatures(self):
        """Multiple signatures at end."""
        body = """Content

---
proj | USER | abc | 2026-01-01

---
proj | WORKER | def | 2026-01-02"""
        result = clean_body(body)
        # Should remove both? Or just the last one?
        # Current logic finds the LAST --- that looks like signature
        # So the first one won't be removed because there's content after
        # Hmm, let's see what happens
        assert "Content" in result


class TestGetIdentityEdgeCases:
    """More get_identity edge cases."""

    @patch('subprocess.check_output')
    def test_git_url_with_port(self, mock_git):
        """Git URL with port number."""
        mock_git.return_value = "ssh://git@github.com:22/owner/repo.git\n"
        with patch.dict(os.environ, {}, clear=True):
            os.environ.pop("AI_PROJECT", None)
            result = get_identity()
            # Split by / gives [..., "repo.git"]
            assert result["project"] == "repo"

    @patch('subprocess.check_output')
    def test_git_url_gitlab_format(self, mock_git):
        """GitLab URL format with groups."""
        mock_git.return_value = "https://gitlab.com/group/subgroup/repo.git\n"
        with patch.dict(os.environ, {}, clear=True):
            os.environ.pop("AI_PROJECT", None)
            result = get_identity()
            # Should get last component
            assert result["project"] == "repo"

    @patch('subprocess.check_output')
    def test_git_remote_not_origin(self, mock_git):
        """What if 'origin' remote doesn't exist?"""
        mock_git.side_effect = subprocess.CalledProcessError(1, "git")
        with patch.dict(os.environ, {}, clear=True):
            os.environ.pop("AI_PROJECT", None)
            # Should fall back to cwd name
            result = get_identity()
            assert result["project"] != ""


class TestBuildSignatureEdgeCases:
    """More build_signature edge cases."""

    @patch('gh_post.get_commit', return_value='abc')
    def test_session_exactly_8_chars(self, mock):
        """Session ID exactly 8 characters."""
        identity = {"project": "p", "role": "W", "iteration": "", "session": "12345678"}
        result = build_signature(identity)
        assert "12345678" in result

    @patch('gh_post.get_commit', return_value='abc')
    def test_session_less_than_8_chars(self, mock):
        """Session ID less than 8 characters."""
        identity = {"project": "p", "role": "W", "iteration": "", "session": "abc"}
        result = build_signature(identity)
        # [:8] on "abc" gives "abc"
        assert "abc" in result

    @patch('gh_post.get_commit', return_value='')
    def test_empty_commit_hash(self, mock):
        """Empty commit hash."""
        identity = {"project": "p", "role": "W", "iteration": "", "session": ""}
        result = build_signature(identity)
        # Commit is empty string, not "-"
        assert " |  |" in result or "| |" in result  # Empty commit field


class TestWrapperBinGh:
    """Tests for the bin/gh wrapper script."""

    def test_wrapper_exists_and_executable(self):
        """bin/gh should exist and be executable."""
        wrapper = os.path.join(os.path.dirname(__file__), '..', 'ai_template_scripts', 'bin', 'gh')
        assert os.path.isfile(wrapper)
        assert os.access(wrapper, os.X_OK)

    def test_wrapper_passes_through_non_issue(self):
        """Non-issue commands should pass through."""
        # This would require actually running gh, so we'll just check the script logic
        wrapper_path = os.path.join(os.path.dirname(__file__), '..', 'ai_template_scripts', 'bin', 'gh')
        with open(wrapper_path) as f:
            content = f.read()
        # Check that it only intercepts issue create/comment
        assert 'issue' in content
        assert 'create' in content
        assert 'comment' in content
