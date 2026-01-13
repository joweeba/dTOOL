# Quality Audit Methodologies

Reference for workers executing quality rotation tasks assigned by manager.

## Code Quality Audit

Find issues with complexity, duplication, dead code.

**Tools:** `./ai_template_scripts/code_stats.py [path]`

**Targets:** Functions >10 cyclomatic complexity, duplicated logic, unused code paths.

## Test Gaps Audit

Find gaps: mocks instead of real implementations, missing edge cases, no integration tests.

**Check for:**
- Tests that mock what they should test
- Missing error path coverage
- No end-to-end tests for critical flows

## Anti-Patterns Audit

Check `postmortems/` for known failure patterns, find violations in current code.

**Process:**
1. `grep -r "keyword" postmortems/` for relevant history
2. Search codebase for same patterns
3. File issues for violations found

## Refactoring Audit

Find files needing cleanup: large files, unclear logic, inefficient algorithms.

**Signals:** Files >500 lines, functions >50 lines, O(n²) where O(n) exists.

## Docs Audit

Audit prompts, docs, and scripts for quality issues.

**Major categories** (find 3+ each):

| Category | What to look for |
|----------|------------------|
| Inconsistencies | Contradictions between files, mismatched behavior vs docs |
| Verbosity | Duplicated explanations, unnecessary detail, stale data |
| Errors | Bugs, undefined vars, wrong line refs, broken logic |
| Confusing | Unclear terms, unexplained concepts, ambiguous instructions |
| Incomplete | Missing docs, undefined labels, undocumented scripts |

**Minor categories** (bonus):

| Category | What to look for |
|----------|------------------|
| Stale references | Line numbers, file paths, commit refs that are outdated |
| Hardcoded values | Org names, repo IDs, URLs that should be parameterized |
| Silent errors | `except:` clauses, `2>/dev/null`, swallowed failures |
| Missing validation | Unchecked config keys, unvalidated inputs |
| Naming inconsistencies | Mixed conventions (snake_case vs kebab-case) |
| Documentation gaps | Missing READMEs, unexplained schemas |
| Dead code | "Not yet integrated", unused files still synced |

**Process:** File one jumbo issue with all findings, then iterate to fix.

---

## Audit Rules (All Types)

- Find at least 3 issues per audit
- If found, loop until <3 remain
- If not found, rigorously explain why not
- File issues for problems: `gh issue create --title "Task" --body "..." --label P2`
