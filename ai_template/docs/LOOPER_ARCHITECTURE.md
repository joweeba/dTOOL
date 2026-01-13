# looper.py Architecture

> Quick reference for the 1260-line autonomous AI loop runner.

## Overview

`looper.py` runs continuous AI sessions for WORKER, PROVER, RESEARCHER, and MANAGER roles. Each iteration:
1. Builds a dynamic prompt with injected context
2. Runs claude or codex with the prompt
3. Streams output through json_to_text.py
4. Waits before next iteration

## Key Components

### Configuration (lines 40-85)

```python
parse_frontmatter(content) -> (config, body)  # Parse .claude/roles/*.md YAML
load_role_config(mode) -> (config, prompt)    # Load shared.md + {role}.md
```

Role config from frontmatter:
- `restart_delay` - Seconds between iterations
- `error_delay` - Seconds after crash
- `iteration_timeout` - Max seconds per iteration
- `codex_interval` - Use Codex every N iterations (0=never)
- `git_author_name` - Identity for commits
- `rotation_type` - "audit" or "research" (enables rotation)
- `rotation_phases` - List of focus areas to cycle through

### Session Injection (lines 86-385)

Builds dynamic content for `<!-- INJECT:key -->` markers in prompts:

```python
run_session_start_commands(role) -> dict  # Orchestrates injection
├── git_log: git log --oneline -10
├── gh_issues: _get_sampled_issues()      # Priority-sorted sample
├── last_directive: _get_last_directive() # ## Next from last same-role commit
├── other_feedback: _get_other_role_feedback()  # Recent commits from other roles
└── rotation_focus: get_rotation_focus()  # Freeform vs focused phase
```

Issue sampling (lines 130-224):
- All in-progress (up to 5)
- All P0
- Top 3 P1, top 2 P2
- 2 newest, 1 random, 1 oldest

### Git Hooks (lines 416-494)

Hooks installed to `.git/hooks/`:
- `pre-commit` - Ruff check, shellcheck, sensitive file blocking
- `commit-msg` - Auto-adds [W]N: prefix, validates structure

### LoopRunner Class (lines 497-1231)

```
LoopRunner(mode)
├── setup()              # Init env, git identity, signal handlers
├── run()                # Main loop
│   └── run_iteration()  # Single AI iteration
│       ├── check_hint()         # Read HINT.md
│       ├── select_ai_tool()     # Claude vs Codex
│       └── (subprocess)         # Run AI with streaming
├── write_status()       # Write .{role}_status.json
├── check_session_success()  # Did session commit?
└── log_crash()          # Write to crashes.log
```

### Git Identity (lines 551-597)

Format: `{project}-{role}-{iteration} <{session}@{machine}.{project}.ai-fleet>`

Environment variables set:
- `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL` - For commits
- `AI_PROJECT`, `AI_ROLE`, `AI_ITERATION`, `AI_SESSION` - For tools
- `AI_CODER`, `CLAUDE_CODE_VERSION` - For commit signatures

### AI Tool Selection (lines 884-894)

```python
def select_ai_tool():
    if codex_interval > 0 and iteration % codex_interval == 0:
        return "codex"
    return "claude"
```

### HINT.md Handling (lines 775-841)

1. Check for `./HINT.md`
2. Read and delete it
3. Log to `HINTS_HISTORY.log`
4. Write `HINT_ACK.md` for manager
5. Warn if hints arriving faster than 30 min

## File Outputs

| File | Purpose | Lifecycle |
|------|---------|-----------|
| `.{role}_status.json` | Real-time status | Updated each iteration, deleted on exit |
| `worker_logs/*.jsonl` | Full AI output | Rotated, keeps last 50 |
| `worker_logs/crashes.log` | Crash history | Rotated, keeps 500 lines |
| `HINTS_HISTORY.log` | HINT.md log | Append-only |
| `HINT_ACK.md` | Acknowledgment | Overwritten each hint |

## Stopping the Loop

- `touch STOP` - Graceful shutdown (checked each iteration)
- `Ctrl+C` - SIGINT handler terminates current process
- Kill PID from `.pid_{role}`
