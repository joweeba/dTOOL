"""
Tests for spawn_session.sh - iTerm2 session spawning.

Tests cover argument parsing, validation, error handling, and dry-run mode.
Since the script interacts with iTerm2, we primarily test via --dry-run
and error cases.
"""

import subprocess
import tempfile
from pathlib import Path

SCRIPT_PATH = Path(__file__).parent.parent / "ai_template_scripts" / "spawn_session.sh"


def run_spawn(args: list[str], cwd: str = None, expect_fail: bool = False) -> tuple[int, str, str]:
    """Run spawn_session.sh and return (returncode, stdout, stderr)."""
    result = subprocess.run(
        [str(SCRIPT_PATH)] + args,
        capture_output=True, text=True, cwd=cwd
    )
    if not expect_fail and result.returncode != 0:
        # For debugging
        print(f"STDOUT: {result.stdout}")
        print(f"STDERR: {result.stderr}")
    return result.returncode, result.stdout, result.stderr


class TestHelpFlag:
    """Tests for --help flag."""

    def test_help_shows_usage(self):
        code, out, err = run_spawn(["--help"])
        assert code == 0
        assert "Usage:" in out
        assert "spawn_session.sh" in out

    def test_help_shows_modes(self):
        code, out, err = run_spawn(["--help"])
        assert "worker" in out
        assert "manager" in out

    def test_help_shows_options(self):
        code, out, err = run_spawn(["--help"])
        assert "--dry-run" in out
        assert "--help" in out

    def test_h_short_flag(self):
        code, out, err = run_spawn(["-h"])
        assert code == 0
        assert "Usage:" in out


class TestDryRunMode:
    """Tests for --dry-run mode."""

    def test_dry_run_worker_current_dir(self):
        """Dry run in current directory defaults to worker."""
        cwd = Path(__file__).parent.parent  # ai_template root (has looper.py)
        code, out, err = run_spawn(["--dry-run"], cwd=str(cwd))
        assert code == 0
        assert "Would create iTerm2 tab:" in out
        assert "[W]" in out
        assert "Would run:" in out
        assert "looper.py worker" in out

    def test_dry_run_manager(self):
        """Dry run manager mode."""
        cwd = Path(__file__).parent.parent
        code, out, err = run_spawn(["--dry-run", "manager"], cwd=str(cwd))
        assert code == 0
        assert "[M]" in out
        assert "looper.py manager" in out

    def test_dry_run_explicit_worker(self):
        """Dry run explicit worker mode."""
        cwd = Path(__file__).parent.parent
        code, out, err = run_spawn(["--dry-run", "worker"], cwd=str(cwd))
        assert code == 0
        assert "[W]" in out
        assert "looper.py worker" in out

    def test_dry_run_with_path(self):
        """Dry run with explicit path."""
        project = Path(__file__).parent.parent  # ai_template
        code, out, err = run_spawn(["--dry-run", "worker", str(project)])
        assert code == 0
        assert "ai_template" in out
        assert "[W]ai_template" in out

    def test_dry_run_swapped_args(self):
        """Dry run with path before mode (swapped order)."""
        project = Path(__file__).parent.parent
        code, out, err = run_spawn(["--dry-run", str(project), "worker"])
        assert code == 0
        assert "[W]ai_template" in out

    def test_dry_run_path_only(self):
        """Dry run with only path (defaults to worker)."""
        project = Path(__file__).parent.parent
        code, out, err = run_spawn(["--dry-run", str(project)])
        assert code == 0
        assert "[W]" in out
        assert "looper.py worker" in out


class TestArgumentParsing:
    """Tests for argument parsing edge cases."""

    def test_too_many_args(self):
        """More than 2 positional args should error."""
        code, out, err = run_spawn(["worker", "/tmp", "extra"], expect_fail=True)
        assert code != 0
        assert "Too many arguments" in err

    def test_unknown_option(self):
        """Unknown options should error."""
        code, out, err = run_spawn(["--unknown"], expect_fail=True)
        assert code != 0
        assert "Unknown option" in err

    def test_invalid_mode(self):
        """Invalid mode should error."""
        cwd = Path(__file__).parent.parent
        code, out, err = run_spawn(["--dry-run", "invalid"], cwd=str(cwd), expect_fail=True)
        assert code != 0
        assert "Invalid" in err


class TestPathValidation:
    """Tests for project path validation."""

    def test_nonexistent_path(self):
        """Non-existent path should error."""
        code, out, err = run_spawn(["worker", "/nonexistent/path"], expect_fail=True)
        assert code != 0
        assert "does not exist" in err

    def test_missing_looper(self):
        """Path without looper.py should error."""
        with tempfile.TemporaryDirectory() as tmpdir:
            code, out, err = run_spawn(["--dry-run", "worker", tmpdir], expect_fail=True)
            assert code != 0
            assert "looper.py not found" in err

    def test_non_executable_looper(self):
        """Non-executable looper.py should error."""
        with tempfile.TemporaryDirectory() as tmpdir:
            # Create non-executable looper.py
            looper_file = Path(tmpdir) / "looper.py"
            looper_file.write_text("#!/usr/bin/env python3\n")
            looper_file.chmod(0o644)  # Read but not execute

            code, out, err = run_spawn(["--dry-run", "worker", tmpdir], expect_fail=True)
            assert code != 0
            assert "not executable" in err

    def test_path_with_spaces(self):
        """Paths with spaces should work."""
        with tempfile.TemporaryDirectory(prefix="test project ") as tmpdir:
            # Create executable looper.py
            looper_file = Path(tmpdir) / "looper.py"
            looper_file.write_text("#!/usr/bin/env python3\n")
            looper_file.chmod(0o755)

            code, out, err = run_spawn(["--dry-run", "worker", tmpdir])
            # Will fail on iTerm2 check, but should at least get past path validation
            # or succeed in dry-run
            assert "looper.py not found" not in err


class TestModeDetection:
    """Tests for smart mode/path detection."""

    def test_worker_keyword(self):
        """'worker' should be recognized as mode."""
        cwd = Path(__file__).parent.parent
        code, out, err = run_spawn(["--dry-run", "worker"], cwd=str(cwd))
        assert code == 0
        assert "looper.py worker" in out

    def test_manager_keyword(self):
        """'manager' should be recognized as mode."""
        cwd = Path(__file__).parent.parent
        code, out, err = run_spawn(["--dry-run", "manager"], cwd=str(cwd))
        assert code == 0
        assert "looper.py manager" in out

    def test_path_detected_as_path(self):
        """Existing directory should be detected as path."""
        project = Path(__file__).parent.parent
        code, out, err = run_spawn(["--dry-run", str(project)])
        assert code == 0
        # Should detect it's a path and default to worker
        assert "looper.py worker" in out

    def test_path_before_mode_detected(self):
        """Path before mode should be correctly detected."""
        project = Path(__file__).parent.parent
        code, out, err = run_spawn(["--dry-run", str(project), "manager"])
        assert code == 0
        assert "looper.py manager" in out


class TestTabTitleSanitization:
    """Tests for tab title sanitization."""

    def test_normal_project_name(self):
        """Normal project name in tab title."""
        cwd = Path(__file__).parent.parent
        code, out, err = run_spawn(["--dry-run", "worker"], cwd=str(cwd))
        assert code == 0
        assert "[W]ai_template" in out

    def test_project_name_with_hyphen(self):
        """Project name with hyphen should work."""
        with tempfile.TemporaryDirectory(prefix="my-project-") as tmpdir:
            looper_file = Path(tmpdir) / "looper.py"
            looper_file.write_text("#!/usr/bin/env python3\n")
            looper_file.chmod(0o755)

            code, out, err = run_spawn(["--dry-run", "worker", tmpdir])
            assert code == 0
            assert "[W]my-project" in out

    def test_project_name_with_underscore(self):
        """Project name with underscore should work."""
        with tempfile.TemporaryDirectory(prefix="my_project_") as tmpdir:
            looper_file = Path(tmpdir) / "looper.py"
            looper_file.write_text("#!/usr/bin/env python3\n")
            looper_file.chmod(0o755)

            code, out, err = run_spawn(["--dry-run", "worker", tmpdir])
            assert code == 0
            assert "[W]my_project" in out


class TestRoleMapping:
    """Tests for mode to role letter mapping."""

    def test_worker_maps_to_w(self):
        cwd = Path(__file__).parent.parent
        code, out, err = run_spawn(["--dry-run", "worker"], cwd=str(cwd))
        assert "[W]" in out

    def test_manager_maps_to_m(self):
        cwd = Path(__file__).parent.parent
        code, out, err = run_spawn(["--dry-run", "manager"], cwd=str(cwd))
        assert "[M]" in out


class TestErrorMessages:
    """Tests for helpful error messages."""

    def test_invalid_mode_suggests_valid(self):
        cwd = Path(__file__).parent.parent
        code, out, err = run_spawn(["--dry-run", "supervisor"], cwd=str(cwd), expect_fail=True)
        assert code != 0
        assert "worker" in err.lower() or "manager" in err.lower() or "Invalid" in err

    def test_missing_looper_suggests_check(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            code, out, err = run_spawn(["worker", tmpdir], expect_fail=True)
            assert code != 0
            assert "looper.py" in err

    def test_nonexistent_path_shows_path(self):
        code, out, err = run_spawn(["worker", "/path/to/nowhere"], expect_fail=True)
        assert code != 0
        assert "/path/to/nowhere" in err or "does not exist" in err
