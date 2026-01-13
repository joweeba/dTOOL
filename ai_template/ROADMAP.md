# Roadmap: ai_template

> Template infrastructure for 45+ AI-driven repos in dropbox org.

## Current State: Clean

- **Open issues:** 1
- **Lint errors:** 0
- **Tests:** 710 passing

## Active Work

### Open Issue
- [ ] #132 - Claude Code session incorrectly inferred [R] role instead of [U]

### Recently Fixed
- [x] #130 - sync_repo.sh overwrites project-specific gitignore entries (fixed in c6da885)
- [x] #131 - AIs don't know to use looper.py for agent work (docs added in 61b896b)

### Deferred to Other Repos
- dterm#5: Long-running background process management
- dasher#4: Cross-repo sync tools
- leadership#52: Remove stale commit counts from org_chart.md (closed)

## Recently Completed

### Lint Cleanup & Documentation (U42-U44) - DONE
- [x] All 12 remaining #118 audit items addressed
- [x] Created docs/LOOPER_ARCHITECTURE.md (100-line quick reference)
- [x] Fixed 61 lint issues across repo (looper.py + tests)
- [x] Closed stale #129 (sync already done)

### P0: Critical Bug Fixes (#118) - DONE
- [x] Fix gh wrapper SCRIPT_DIR bug
- [x] Fix looper.py iteration pattern for all roles
- [x] Fix health_check.py role commit counting
- [x] Fix bg_task.py env var mismatch
- [x] Fix pulse.py closed issue count
- [x] Fix postmortem template checkbox violation

### P0: Core DNA - Lineage & Authorship (#127, #128) - DONE
- [x] Andrew Yates identity in ai_template.md
- [x] Lineage section in commit template
- [x] Copyright header templates
- [x] Pre-commit hook (warning mode) for headers
- [x] Author validation in pre-commit and sync_repo.sh
- [x] andrewdyates alias recognized

### P1: Framework Improvements (tla2 mail) - DONE
- [x] #119 - Baseline alignment guidance (verification repos)
- [x] #120 - Manager investigation limits + researcher handoff
- [x] #121 - Worker WIP commit guidance
- [x] #122 - Long-running operations must run in background
- [x] #123 - Skip-tests anti-pattern added
- [x] #124, #125 - Acknowledged researcher findings
- [x] #126 - Dash News style guidance for acronyms

## When to Update ai_template

- Bug fixes in template infrastructure
- New patterns that benefit all projects
- Rule clarifications after postmortems
- Field learnings from active repos
