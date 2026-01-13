# AI Template

Template repository for autonomous AI-driven software development.

**Author:** Andrew Yates · **Copyright:** 2026 Dropbox, Inc. · **License:** Apache 2.0

## What This Is

A git template for autonomous AI-driven development. AI agents work in specialized roles, claim GitHub issues, and coordinate via git. Git is the source of truth.

## Roles

| Role | Tag | Mission | Loop Interval |
|------|-----|---------|---------------|
| **WORKER** | `[W]` | Write code. Build the system. | Continuous |
| **PROVER** | `[P]` | Write proofs. Prove it works. | 15 min |
| **RESEARCHER** | `[R]` | Study, document, and design. | 10 min |
| **MANAGER** | `[M]` | Audit progress and direct others. | 15 min |

**How they interact:**
```
RESEARCHER informs what to build
    ↓
WORKER builds it (flags needs-review when done)
    ↓
PROVER proves it works
    ↓
MANAGER verifies and closes issues
    ↓
RESEARCHER reviews gaps (built vs should-have-built)
    ↓
(loop)
```

**Key boundaries:**
- **Only MANAGER closes issues** - Workers flag completion with `needs-review` label
- **CLAUDE.md is sacred** - Only User edits it (mission/config, not status tracking)
- **Role-specific prompts** - See `.claude/roles/{worker,prover,researcher,manager}.md`

**MANAGER** coordinates all roles, audits claims, directs via HINT.md.

## Quick Start

1. Clone this template for your new project
2. Run `./ai_template_scripts/init_from_template.sh`
3. Complete `CLAUDE.md` with project-specific configuration
4. Create issues with `gh issue create` or write `ROADMAP.md`
5. Start worker: `./looper.py worker`

## File Reference

### Root Files

| File | Purpose | Who Uses It |
|------|---------|-------------|
| `CLAUDE.md` | Project config and AI instructions | Claude Code |
| `AGENTS.md` | Redirects to CLAUDE.md | OpenAI Codex |
| `GEMINI.md` | Redirects to CLAUDE.md | Google Gemini |
| `looper.py` | Autonomous AI role loop | Human (starts it) |
| `ROADMAP.md` | Current project roadmap | AI + Human |
| `IDEAS.md` | Future ideas (not actionable) | Human |
| `HINTS_HISTORY.log` | Log of HINT.md messages | looper.py |
| `requirements.txt` | Python dependencies | pip |
| `ruff.toml` | Python linter config | ruff |
| `LICENSE` | Apache 2.0 license | Legal |

### Documentation Structure

| Location | Purpose | Pattern |
|----------|---------|---------|
| `ROADMAP.md` | Active work - what we're doing now | Single file, update in place |
| `IDEAS.md` | Future backlog - not actionable yet | Single file, update in place |
| `designs/` | Design records - historical log | Dated: `YYYY-MM-DD-slug.md` |
| `docs/` | Reference material - evergreen | Update in place, not dated |
| `reports/` | Session snapshots, investigations | Dated, prune after 60 days |
| `postmortems/` | Failure analysis and learnings | Dated, keep permanently |

**Key distinction:** `designs/` accumulates dated records (grep-friendly history), while `docs/` contains current reference material updated in place.

### AI Rules (`.claude/rules/`)

| File | Purpose | Who Uses It |
|------|---------|-------------|
| `ai_template.md` | Infrastructure rules (roles, workflow, anti-patterns) | Claude Code (auto-loaded) |

### Scripts (`ai_template_scripts/`)

See [`ai_template_scripts/README.md`](ai_template_scripts/README.md) for details on each script.

### Tests (`tests/`)

| File | Purpose |
|------|---------|
| `test_*.py` | Unit tests for scripts |
| `conftest.py` | Pytest fixtures |

### Generated (gitignored)

| Path | Purpose |
|------|---------|
| `worker_logs/` | Iteration logs and crash history |
| `.background_tasks/` | Background task state |
| `__pycache__/`, `.mypy_cache/`, etc | Tool caches |

## How It Works

See [`.claude/rules/ai_template.md`](.claude/rules/ai_template.md) for workflow details.
