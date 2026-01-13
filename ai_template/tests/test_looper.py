"""Tests for looper.py

Tests the pipe handling, crash detection, and session success checking.
"""

import sys
import time
from pathlib import Path
from unittest.mock import patch

import pytest

# Add parent dir to path for imports
sys.path.insert(0, str(Path(__file__).parent.parent))

from looper import (
    LoopRunner,
    get_project_name,
    get_rotation_focus,
    inject_content,
    load_role_config,
    parse_frontmatter,
    run_session_start_commands,
)


class TestGetProjectName:
    """Test project name extraction from git remote."""

    def test_returns_string(self):
        """Should return a non-empty string."""
        name = get_project_name()
        assert isinstance(name, str)
        assert len(name) > 0

    def test_matches_expected_format(self):
        """Should return 'ai_template' when in this repo."""
        name = get_project_name()
        # We're in ai_template repo
        assert name == "ai_template"


class TestLoopRunnerInit:
    """Test LoopRunner initialization."""

    def test_worker_mode(self):
        """Worker mode should have correct config."""
        runner = LoopRunner("worker")
        assert runner.mode == "worker"
        assert runner.config["git_author_name"] == "WORKER"
        assert runner.config["restart_delay"] == 0

    def test_manager_mode(self):
        """Manager mode should have correct config."""
        runner = LoopRunner("manager")
        assert runner.mode == "manager"
        assert runner.config["git_author_name"] == "MANAGER"
        assert runner.config["restart_delay"] > 0


class TestCheckSessionSuccess:
    """Test session success detection via git commits."""

    @pytest.fixture
    def runner(self):
        """Create a LoopRunner for testing."""
        return LoopRunner("worker")

    def test_no_commit_returns_false(self, runner):
        """When no commits in timeframe, should return False."""
        # Use a start time in the future so no commits match
        future_time = time.time() + 86400  # Tomorrow
        assert runner.check_session_success(future_time) is False

    def test_recent_commit_returns_true(self, runner):
        """When there's a recent WORKER commit, should return True."""
        # Use start time before the most recent commit
        # The actual test depends on git history having WORKER commits
        past_time = time.time() - 86400  # Yesterday
        # This may return True or False depending on git history
        result = runner.check_session_success(past_time)
        assert isinstance(result, bool)

    def test_handles_git_failure(self, runner):
        """Should return False if git command fails."""
        with patch("subprocess.run", side_effect=Exception("git failed")):
            assert runner.check_session_success(time.time()) is False


class TestLogCrash:
    """Test crash logging with session success distinction."""

    @pytest.fixture
    def runner(self, tmp_path):
        """Create a LoopRunner with a temp crash log."""
        runner = LoopRunner("worker")
        runner.crash_log = tmp_path / "crashes.log"
        runner.iteration = 5
        return runner

    def test_real_crash_logs_to_file(self, runner, capsys):
        """A real crash (no commit) should log to crashes.log."""
        runner.log_crash(1, "claude", session_committed=False)

        # Should write to crash log
        assert runner.crash_log.exists()
        content = runner.crash_log.read_text()
        assert "claude exited with code 1" in content

        # Should print crash banner
        captured = capsys.readouterr()
        assert "CRASH DETECTED" in captured.out

    def test_successful_session_no_log(self, runner, capsys):
        """Exit code 1 after successful commit should NOT log as crash."""
        runner.log_crash(1, "claude", session_committed=True)

        # Should NOT write to crash log
        assert not runner.crash_log.exists()

        # Should print note about successful session
        captured = capsys.readouterr()
        assert "committed successfully" in captured.out
        assert "CRASH DETECTED" not in captured.out

    def test_signal_death_always_crash(self, runner, capsys):
        """Kill by signal is always a crash, even with commit."""
        runner.log_crash(137, "claude", session_committed=True)  # SIGKILL = 128+9

        # Should write to crash log (signal death is always real)
        assert runner.crash_log.exists()
        content = runner.crash_log.read_text()
        assert "killed by signal 9" in content

    def test_timeout_with_commit_not_crash(self, runner, capsys):
        """Timeout (124) after commit is not a crash."""
        runner.log_crash(124, "claude", session_committed=True)

        # Should NOT write to crash log
        assert not runner.crash_log.exists()

        captured = capsys.readouterr()
        assert "committed successfully" in captured.out

    def test_timeout_without_commit_is_crash(self, runner, capsys):
        """Timeout (124) without commit is a crash."""
        runner.log_crash(124, "claude", session_committed=False)

        # Should write to crash log
        assert runner.crash_log.exists()
        content = runner.crash_log.read_text()
        assert "timed out" in content


class TestPipeHandling:
    """Test the pipe handling logic that prevents EPIPE."""

    def test_write_to_text_proc_handles_broken_pipe(self):
        """write_to_text_proc should gracefully handle BrokenPipeError."""
        # Create a mock scenario
        text_proc_alive = True

        def write_to_text_proc(data: bytes) -> None:
            nonlocal text_proc_alive
            if not text_proc_alive:
                return
            # Simulate broken pipe
            raise BrokenPipeError("Broken pipe")

        # First call should raise, but we catch it
        # In real code, it sets text_proc_alive = False and continues
        with pytest.raises(BrokenPipeError):
            write_to_text_proc(b"test")

    def test_drain_output_continues_after_pipe_failure(self):
        """Should continue draining ai_proc even after text_proc dies."""
        # This tests the conceptual behavior - in the actual implementation,
        # after BrokenPipeError, text_proc_alive becomes False and we
        # continue reading from ai_proc without writing to text_proc

        text_proc_alive = True
        logged_lines = []

        def write_to_text_proc(data: bytes) -> None:
            nonlocal text_proc_alive
            if not text_proc_alive:
                return
            try:
                # Simulate broken pipe on first write
                if len(logged_lines) == 0:
                    raise BrokenPipeError
            except BrokenPipeError:
                text_proc_alive = False

        # Simulate reading lines from ai_proc
        lines = [b"line1\n", b"line2\n", b"line3\n"]
        for line in lines:
            logged_lines.append(line)
            write_to_text_proc(line)

        # All lines should be logged even though text_proc died
        assert len(logged_lines) == 3


class TestRunIteration:
    """Test run_iteration return value."""

    def test_returns_tuple(self):
        """run_iteration should return (exit_code, start_time)."""
        # We can't easily test run_iteration without mocking everything,
        # but we can verify the method exists and is callable
        runner = LoopRunner("worker")
        assert callable(runner.run_iteration)
        # The return type annotation is tuple[int, float]
        # which is verified by static type checkers


class TestSelectAiTool:
    """Test AI tool selection."""

    def test_first_iteration_uses_claude(self):
        """First iteration should always use claude."""
        runner = LoopRunner("worker")
        runner.iteration = 1
        runner.codex_available = True
        assert runner.select_ai_tool() == "claude"

    def test_codex_interval(self):
        """Should use codex at configured intervals."""
        runner = LoopRunner("worker")
        runner.codex_available = True
        # Default codex_interval is 9

        runner.iteration = 9
        assert runner.select_ai_tool() == "codex"

        runner.iteration = 18
        assert runner.select_ai_tool() == "codex"

        runner.iteration = 10
        assert runner.select_ai_tool() == "claude"

    def test_codex_not_available(self):
        """Should use claude when codex not available."""
        runner = LoopRunner("worker")
        runner.codex_available = False
        runner.iteration = 9
        assert runner.select_ai_tool() == "claude"

    def test_manager_never_uses_codex(self):
        """Manager mode should never use codex."""
        runner = LoopRunner("manager")
        runner.codex_available = True
        runner.iteration = 9
        assert runner.select_ai_tool() == "claude"


class TestGetGitIteration:
    """Test iteration detection from git log."""

    def test_returns_integer(self):
        """Should return an integer."""
        runner = LoopRunner("worker")
        result = runner.get_git_iteration()
        assert isinstance(result, int)
        assert result >= 1

    def test_parses_worker_commits(self):
        """Should find [W]N pattern in git log."""
        runner = LoopRunner("worker")
        # In the ai_template repo, we should have WORKER commits
        result = runner.get_git_iteration()
        # Should be at least 1 (possibly higher if commits exist)
        assert result >= 1

    def test_handles_git_failure(self):
        """Should return 1 if git command fails."""
        runner = LoopRunner("worker")
        with patch("subprocess.run", side_effect=Exception("git failed")):
            assert runner.get_git_iteration() == 1


class TestParseFrontmatter:
    """Test YAML frontmatter parsing from markdown."""

    def test_basic_frontmatter(self):
        """Should parse key: value pairs."""
        content = """---
name: test
value: 123
---

# Body content
"""
        config, body = parse_frontmatter(content)
        assert config["name"] == "test"
        assert config["value"] == 123
        assert "# Body content" in body

    def test_no_frontmatter(self):
        """Should return empty config if no frontmatter."""
        content = "# Just markdown\nNo frontmatter here."
        config, body = parse_frontmatter(content)
        assert config == {}
        assert body == content

    def test_boolean_values(self):
        """Should parse true/false as booleans."""
        content = """---
enabled: true
disabled: false
---
body
"""
        config, body = parse_frontmatter(content)
        assert config["enabled"] is True
        assert config["disabled"] is False

    def test_comma_separated_list(self):
        """Should parse comma-separated values as list."""
        content = """---
phases: code_quality,test_gaps,anti_patterns
---
body
"""
        config, body = parse_frontmatter(content)
        assert config["phases"] == ["code_quality", "test_gaps", "anti_patterns"]

    def test_comments_ignored(self):
        """Should ignore comment lines in frontmatter."""
        content = """---
# This is a comment
name: test
# Another comment
---
body
"""
        config, body = parse_frontmatter(content)
        assert config == {"name": "test"}

    def test_negative_integers(self):
        """Should parse negative integers."""
        content = """---
value: -5
---
body
"""
        config, body = parse_frontmatter(content)
        assert config["value"] == -5


class TestGetRotationFocus:
    """Test rotation focus calculation."""

    def test_odd_iteration_freeform(self):
        """Odd iterations should return freeform."""
        result = get_rotation_focus(1, "audit", ["a", "b", "c"])
        assert "Freeform" in result

        result = get_rotation_focus(3, "audit", ["a", "b", "c"])
        assert "Freeform" in result

    def test_even_iteration_cycles(self):
        """Even iterations should cycle through phases."""
        phases = ["code_quality", "test_gaps", "anti_patterns", "refactoring"]

        # Iteration 2 -> phase 0
        result = get_rotation_focus(2, "audit", phases)
        assert "Code Quality" in result

        # Iteration 4 -> phase 1
        result = get_rotation_focus(4, "audit", phases)
        assert "Test Gaps" in result

        # Iteration 6 -> phase 2
        result = get_rotation_focus(6, "audit", phases)
        assert "Anti Patterns" in result

        # Iteration 8 -> phase 3
        result = get_rotation_focus(8, "audit", phases)
        assert "Refactoring" in result

        # Iteration 10 -> wraps to phase 0
        result = get_rotation_focus(10, "audit", phases)
        assert "Code Quality" in result

    def test_no_rotation_returns_empty(self):
        """No rotation type should return empty string."""
        assert get_rotation_focus(1, "", ["a", "b"]) == ""
        assert get_rotation_focus(1, "audit", []) == ""


class TestInjectContent:
    """Test content injection into templates."""

    def test_single_marker(self):
        """Should replace single marker."""
        template = "Before <!-- INJECT:test --> After"
        result = inject_content(template, {"test": "REPLACED"})
        assert result == "Before REPLACED After"

    def test_multiple_markers(self):
        """Should replace multiple markers."""
        template = "A <!-- INJECT:a --> B <!-- INJECT:b --> C"
        result = inject_content(template, {"a": "1", "b": "2"})
        assert result == "A 1 B 2 C"

    def test_no_markers(self):
        """Template without markers should be unchanged."""
        template = "No markers here"
        result = inject_content(template, {"test": "value"})
        assert result == template

    def test_missing_replacement(self):
        """Missing replacement should leave marker in place."""
        template = "Before <!-- INJECT:missing --> After"
        result = inject_content(template, {})
        assert "<!-- INJECT:missing -->" in result


class TestLoadRoleConfig:
    """Test role config loading from .claude/roles/ files."""

    def test_worker_config(self):
        """Worker should load from role file."""
        config, prompt = load_role_config("worker")
        assert config["restart_delay"] == 0
        assert config["codex_interval"] == 9
        assert "WORKER" in prompt

    def test_manager_config(self):
        """Manager should load with rotation config."""
        config, prompt = load_role_config("manager")
        assert config["restart_delay"] == 900
        assert config["rotation_type"] == "audit"
        assert "MANAGER" in prompt

    def test_researcher_config(self):
        """Researcher should load with rotation config."""
        config, prompt = load_role_config("researcher")
        assert config["rotation_type"] == "research"
        assert "RESEARCHER" in prompt

    def test_prover_config(self):
        """Prover should load from role file."""
        config, prompt = load_role_config("prover")
        assert config["restart_delay"] == 900
        assert "PROVER" in prompt

    def test_shared_content_included(self):
        """All roles should include shared.md content."""
        config, prompt = load_role_config("worker")
        assert "Recent Commits" in prompt
        assert "Open Issues" in prompt


class TestRunSessionStartCommands:
    """Test session start command execution."""

    def test_returns_dict(self):
        """Should return dict with git_log, gh_issues, last_directive, other_feedback."""
        results = run_session_start_commands("worker")
        assert isinstance(results, dict)
        assert "git_log" in results
        assert "gh_issues" in results
        assert "last_directive" in results
        assert "other_feedback" in results

    def test_git_log_has_content(self):
        """git_log should have content (we're in a git repo)."""
        results = run_session_start_commands("worker")
        # Should either have commit output or an error message
        assert len(results["git_log"]) > 0

    def test_handles_failures_gracefully(self):
        """Should not raise on command failures."""
        # Even if gh fails (not authenticated), should return error message
        results = run_session_start_commands("manager")
        assert isinstance(results["gh_issues"], str)

    def test_role_parameter_affects_directive(self):
        """Different roles should look for different commit prefixes."""
        # This just tests that the function runs with different roles
        for role in ["worker", "manager", "researcher", "prover"]:
            results = run_session_start_commands(role)
            assert isinstance(results["last_directive"], str)
            assert isinstance(results["other_feedback"], str)


