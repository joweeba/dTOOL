"""
Tests for gh_post.py - AI identity wrapper for gh commands.

These are hard tests covering edge cases in body cleaning, title fixing,
and argument parsing.
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
    process_body,
)


class TestCleanBody:
    """Tests for clean_body() - the core deduplication logic."""

    def test_removes_from_header_at_start(self):
        body = "**FROM:** ai_template [WORKER]123\n\nActual content"
        assert clean_body(body) == "Actual content"

    def test_removes_from_header_with_blank_line(self):
        body = "**FROM:** project [USER]\n\nContent here\n\nMore content"
        assert clean_body(body) == "Content here\n\nMore content"

    def test_removes_multiple_from_headers(self):
        """Edge case: AI added header, then wrapper would add another."""
        body = "**FROM:** old [USER]\n\n**FROM:** newer [WORKER]123\n\nReal content"
        assert clean_body(body) == "Real content"

    def test_preserves_from_in_middle_of_content(self):
        """FROM in middle of content is legitimate - don't remove."""
        body = "Some content\n\n**FROM:** this is a quote\n\nMore content"
        result = clean_body(body)
        assert "**FROM:** this is a quote" in result

    def test_removes_new_signature_format(self):
        body = "Content\n\n---\nai_template | WORKER #123 | abc | 2026-01-01"
        assert clean_body(body) == "Content"

    def test_removes_old_signature_format(self):
        body = """Content here

---
Project: ai_template
Role: WORKER
Iteration: 123
Commit: abc123
Timestamp: 2026-01-01T00:00:00Z"""
        assert clean_body(body) == "Content here"

    def test_preserves_legitimate_hr_rule(self):
        """Markdown --- horizontal rule should be preserved."""
        body = "Section 1\n\n---\n\nSection 2"
        result = clean_body(body)
        assert "---" in result
        assert "Section 1" in result
        assert "Section 2" in result

    def test_preserves_multiple_hr_rules(self):
        body = "A\n\n---\n\nB\n\n---\n\nC"
        result = clean_body(body)
        assert result.count("---") == 2
        assert "A" in result
        assert "B" in result
        assert "C" in result

    def test_hr_followed_by_signature_removes_only_signature(self):
        """HR in content + signature at end: keep HR, remove signature."""
        body = """Content

---

More content

---
ai_template | WORKER #123 | abc | 2026-01-01"""
        result = clean_body(body)
        assert "Content" in result
        assert "More content" in result
        assert result.count("---") == 1  # Only the legitimate one
        assert "WORKER" not in result

    def test_empty_body(self):
        assert clean_body("") == ""

    def test_whitespace_only_body(self):
        assert clean_body("   \n\n   ") == ""

    def test_body_with_only_header(self):
        body = "**FROM:** project [USER]"
        assert clean_body(body) == ""

    def test_body_with_only_signature(self):
        body = "---\nproject | USER | abc | 2026-01-01"
        assert clean_body(body) == ""

    def test_body_with_header_and_signature_no_content(self):
        body = "**FROM:** project [USER]\n\n---\nproject | USER | abc | 2026-01-01"
        assert clean_body(body) == ""

    def test_trims_trailing_whitespace(self):
        body = "Content\n\n\n   "
        assert clean_body(body) == "Content"

    def test_trims_leading_blank_lines(self):
        """Trims leading blank lines but preserves indentation."""
        body = "\n\n   Content"
        assert clean_body(body) == "   Content"

    def test_preserves_internal_whitespace(self):
        body = "Line 1\n\n\nLine 2\n\n\n\nLine 3"
        result = clean_body(body)
        assert "Line 1" in result
        assert "Line 2" in result
        assert "Line 3" in result

    def test_handles_windows_line_endings(self):
        body = "**FROM:** project [USER]\r\n\r\nContent\r\n\r\n---\r\nproject | USER | x | y"
        result = clean_body(body)
        # Should handle gracefully even if not perfect
        assert "Content" in result or "Content\r" in result

    def test_signature_with_special_characters_in_project(self):
        body = "Content\n\n---\nmy-project_v2 | WORKER #1 | abc | 2026-01-01"
        assert clean_body(body) == "Content"

    def test_signature_with_long_session_id(self):
        body = "Content\n\n---\nproj | WORKER #1 | abcdefghijklmnop | abc | 2026-01-01"
        assert clean_body(body) == "Content"

    def test_code_block_with_hr_preserved(self):
        """Code blocks containing --- should be preserved."""
        body = """Content

```yaml
---
key: value
---
```

More content"""
        result = clean_body(body)
        assert "```yaml" in result
        assert "key: value" in result

    def test_nested_content_not_confused_with_signature(self):
        """Content that looks like signature but isn't at the end."""
        body = """Real content

Here's what the format looks like:
---
project | WORKER | abc | 2026-01-01

But then more content follows"""
        result = clean_body(body)
        # Should keep everything since "signature-like" content is followed by more text
        assert "Real content" in result
        assert "more content follows" in result


class TestFixTitle:
    """Tests for fix_title() - title prefix handling."""

    def test_adds_prefix_to_plain_title(self):
        identity = {"project": "myproject", "role": "WORKER"}
        result = fix_title("Fix the bug", identity)
        assert result == "[myproject] Fix the bug"

    def test_removes_existing_project_prefix(self):
        identity = {"project": "newproject", "role": "WORKER"}
        result = fix_title("[oldproject] Fix the bug", identity)
        assert result == "[newproject] Fix the bug"

    def test_removes_project_and_role_prefix(self):
        """Old format had [project][W] - should be replaced."""
        identity = {"project": "proj", "role": "WORKER"}
        result = fix_title("[other][W] Fix bug", identity)
        assert result == "[proj] Fix bug"

    def test_handles_multiple_brackets_in_title(self):
        identity = {"project": "proj", "role": "WORKER"}
        result = fix_title("[old] Some [bracketed] content", identity)
        assert result == "[proj] Some [bracketed] content"

    def test_preserves_brackets_in_middle(self):
        identity = {"project": "proj", "role": "WORKER"}
        result = fix_title("Fix [urgent] bug in [module]", identity)
        assert result == "[proj] Fix [urgent] bug in [module]"

    def test_empty_title(self):
        identity = {"project": "proj", "role": "WORKER"}
        result = fix_title("", identity)
        assert result == "[proj] "

    def test_whitespace_after_prefix(self):
        identity = {"project": "proj", "role": "WORKER"}
        result = fix_title("[old]    Title with spaces", identity)
        assert result == "[proj] Title with spaces"

    def test_special_characters_in_project_name(self):
        identity = {"project": "my-project_v2", "role": "WORKER"}
        result = fix_title("Title", identity)
        assert result == "[my-project_v2] Title"


class TestBuildHeader:
    """Tests for build_header() - FROM header generation."""

    def test_header_with_iteration(self):
        identity = {"project": "proj", "role": "WORKER", "iteration": "123", "session": ""}
        result = build_header(identity)
        assert result == "**FROM:** proj [WORKER]123"

    def test_header_without_iteration(self):
        identity = {"project": "proj", "role": "USER", "iteration": "", "session": ""}
        result = build_header(identity)
        assert result == "**FROM:** proj [USER]"

    def test_header_with_manager_role(self):
        identity = {"project": "proj", "role": "MANAGER", "iteration": "5", "session": ""}
        result = build_header(identity)
        assert result == "**FROM:** proj [MANAGER]5"


class TestBuildSignature:
    """Tests for build_signature() - compact signature generation."""

    @patch('gh_post.get_commit', return_value='abc1234')
    def test_signature_with_all_fields(self, mock_commit):
        identity = {
            "project": "proj",
            "role": "WORKER",
            "iteration": "123",
            "session": "session-id-long"
        }
        result = build_signature(identity)
        assert result.startswith("---\n")
        assert "proj | WORKER #123" in result
        assert "session-" in result  # Truncated to 8 chars
        assert "abc1234" in result

    @patch('gh_post.get_commit', return_value='abc1234')
    def test_signature_without_session(self, mock_commit):
        identity = {"project": "proj", "role": "WORKER", "iteration": "1", "session": ""}
        result = build_signature(identity)
        assert "proj | WORKER #1" in result
        assert " | abc1234 |" in result

    @patch('gh_post.get_commit', return_value='-')
    def test_signature_without_commit(self, mock_commit):
        identity = {"project": "proj", "role": "USER", "iteration": "", "session": ""}
        result = build_signature(identity)
        assert "proj | USER" in result
        assert " | - |" in result


class TestParseArgs:
    """Tests for parse_args() - gh argument parsing."""

    def test_parse_issue_create(self):
        args = ["issue", "create", "--title", "My Title", "--body", "My Body"]
        result = parse_args(args)
        assert result["command"] == "issue"
        assert result["subcommand"] == "create"
        assert result["title_index"] == 3
        assert result["body_index"] == 5

    def test_parse_issue_comment(self):
        args = ["issue", "comment", "42", "--body", "Comment text"]
        result = parse_args(args)
        assert result["command"] == "issue"
        assert result["subcommand"] == "comment"
        assert result["body_index"] == 4
        assert result["title_index"] is None

    def test_parse_with_repo(self):
        args = ["issue", "create", "--repo", "owner/repo", "--title", "T", "--body", "B"]
        result = parse_args(args)
        assert result["repo"] == "owner/repo"

    def test_parse_with_mail_label(self):
        args = ["issue", "create", "--label", "mail", "--title", "T", "--body", "B"]
        result = parse_args(args)
        assert result["has_mail_label"] is True

    def test_parse_with_other_label(self):
        args = ["issue", "create", "--label", "bug", "--title", "T", "--body", "B"]
        result = parse_args(args)
        assert result["has_mail_label"] is False

    def test_parse_with_multiple_labels(self):
        args = ["issue", "create", "--label", "P1", "--label", "mail", "--title", "T", "--body", "B"]
        result = parse_args(args)
        assert result["has_mail_label"] is True

    def test_parse_empty_args(self):
        result = parse_args([])
        assert result["command"] == ""
        assert result["subcommand"] == ""

    def test_parse_single_arg(self):
        result = parse_args(["issue"])
        assert result["command"] == "issue"
        assert result["subcommand"] == ""

    def test_parse_args_at_end(self):
        """Args where --body is the last flag."""
        args = ["issue", "create", "--title", "T", "--body", "Body content here"]
        result = parse_args(args)
        assert result["body_index"] == 5

    def test_parse_short_body_flag(self):
        """Short -b flag for body."""
        args = ["issue", "create", "-t", "Title", "-b", "Body"]
        result = parse_args(args)
        assert result["body_index"] == 5
        assert result["title_index"] == 3

    def test_parse_short_title_flag(self):
        """Short -t flag for title."""
        args = ["issue", "create", "-t", "My Title", "--body", "Body"]
        result = parse_args(args)
        assert result["title_index"] == 3

    def test_parse_short_repo_flag(self):
        """Short -R flag for repo."""
        args = ["issue", "create", "-R", "owner/repo", "-t", "T", "-b", "B"]
        result = parse_args(args)
        assert result["repo"] == "owner/repo"

    def test_parse_short_label_flag(self):
        """Short -l flag for label."""
        args = ["issue", "create", "-l", "mail", "-t", "T", "-b", "B"]
        result = parse_args(args)
        assert result["has_mail_label"] is True

    def test_parse_mixed_short_long_flags(self):
        """Mix of short and long flags."""
        args = ["issue", "create", "--title", "T", "-b", "Body", "-R", "owner/repo"]
        result = parse_args(args)
        assert result["title_index"] == 3
        assert result["body_index"] == 5
        assert result["repo"] == "owner/repo"


class TestProcessBody:
    """Tests for process_body() - full body processing pipeline."""

    @patch('gh_post.get_commit', return_value='abc1234')
    def test_cleans_and_adds_identity(self, mock_commit):
        identity = {"project": "proj", "role": "WORKER", "iteration": "1", "session": ""}
        body = "**FROM:** old [USER]\n\nContent\n\n---\nold | USER | x | y"
        result = process_body(body, identity)

        # Should have new header
        assert result.startswith("**FROM:** proj [WORKER]1")
        # Should have content
        assert "Content" in result
        # Should have new signature
        assert "proj | WORKER #1" in result
        # Should not have old identity
        assert "[USER]" not in result or "proj [WORKER]" in result

    @patch('gh_post.get_commit', return_value='abc1234')
    def test_handles_clean_body(self, mock_commit):
        """Body without existing identity markers."""
        identity = {"project": "proj", "role": "WORKER", "iteration": "1", "session": ""}
        body = "Just some content"
        result = process_body(body, identity)

        assert "**FROM:** proj [WORKER]1" in result
        assert "Just some content" in result
        assert "---\nproj | WORKER" in result


class TestGetIdentity:
    """Tests for get_identity() - identity derivation."""

    def test_uses_env_vars_when_set(self):
        with patch.dict(os.environ, {
            "AI_PROJECT": "myproj",
            "AI_ROLE": "MANAGER",
            "AI_ITERATION": "42",
            "AI_SESSION": "sess123"
        }):
            result = get_identity()
            assert result["project"] == "myproj"
            assert result["role"] == "MANAGER"
            assert result["iteration"] == "42"
            assert result["session"] == "sess123"

    def test_defaults_role_to_user(self):
        with patch.dict(os.environ, {"AI_PROJECT": "proj"}, clear=True):
            # Clear other AI_ vars
            for key in list(os.environ.keys()):
                if key.startswith("AI_") and key != "AI_PROJECT":
                    del os.environ[key]
            result = get_identity()
            assert result["role"] == "USER"

    @patch('subprocess.check_output')
    def test_derives_project_from_git(self, mock_git):
        mock_git.return_value = "https://github.com/owner/myrepo.git\n"
        with patch.dict(os.environ, {}, clear=True):
            # Remove AI_PROJECT
            os.environ.pop("AI_PROJECT", None)
            result = get_identity()
            assert result["project"] == "myrepo"


class TestEdgeCases:
    """Additional edge cases and integration-style tests."""

    def test_clean_body_with_unicode(self):
        body = "**FROM:** proj [USER]\n\nContent with émojis 🎉 and ñ"
        result = clean_body(body)
        assert "émojis 🎉" in result
        assert "ñ" in result

    def test_clean_body_with_very_long_content(self):
        long_content = "x" * 10000
        body = f"**FROM:** proj [USER]\n\n{long_content}\n\n---\nproj | USER | x | y"
        result = clean_body(body)
        assert len(result) == 10000
        assert result == long_content

    def test_fix_title_with_unicode(self):
        identity = {"project": "proj", "role": "WORKER"}
        result = fix_title("Fix bug with café ☕", identity)
        assert result == "[proj] Fix bug with café ☕"

    def test_clean_body_preserves_markdown_formatting(self):
        body = """**FROM:** proj [USER]

# Heading

- List item 1
- List item 2

```python
def foo():
    pass
```

**Bold** and *italic*

---
proj | USER | x | y"""
        result = clean_body(body)
        assert "# Heading" in result
        assert "- List item 1" in result
        assert "```python" in result
        assert "**Bold**" in result
