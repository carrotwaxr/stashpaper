---
name: start-session
description: Use when starting a new coding session, beginning work on a branch, or resuming after a context reset. Gathers branch context, in-progress work, plan documents, and recent history to produce a briefing before any code is written.
---

# Start Session

Orientation before action. Gather context from the current branch, git state, plan documents, and recent history, then present a structured briefing.

**Announce at start:** "Using start-session to orient before we begin."

## Step 1: Detect Current State

```bash
# Branch name
git branch --show-current

# Is this main or a feature branch?
# main/master = fresh start, anything else = in-progress work

# Uncommitted changes
git status --short

# Recent commits on this branch (diverged from main)
git log main..HEAD --oneline 2>/dev/null || git log --oneline -5
```

## Step 2: Identify Context Sources

Based on the branch name, find relevant context:

### Plan Documents

```bash
# Search for plan docs matching branch topic
ls docs/plans/ 2>/dev/null | grep -i "<branch-topic-keywords>"
ls docs/plans/ 2>/dev/null | tail -10
```

If a plan doc exists, read it and extract current status, implementation phases, key decisions.

### Recent Git Activity

```bash
# What was the last session working on?
git log --oneline -10 --format="%h %s (%ar)"

# Any stashed work?
git stash list
```

## Step 2b: Query the Brain

Search the MCP memory service for context relevant to the branch topic:

```
memory_search(query: "<branch-topic-keywords>", limit: 5)
memory_search(query: "gotchas stashpaper", limit: 5)
```

Look for prior decisions, known gotchas, patterns established in previous sessions.

## Step 3: Check Project Health

```bash
# Uncommitted changes from a crashed session?
git status --short

# Behind main?
git fetch origin main --quiet 2>/dev/null
git rev-list HEAD..origin/main --count 2>/dev/null
```

## Step 4: Present Briefing

```
## Session Briefing

**Branch**: `<branch-name>` (diverged from main by N commits)
**Status**: <fresh | in-progress | uncommitted changes detected>

### Context
- <Plan doc summary if found, or "No plan doc found">
- <Recent commit summary>

### Brain Context
- <Relevant memories from MCP search, or "No prior context found">

### Relevant Skills
Based on the branch topic, these skills are likely relevant:
- <skill-1>: <why>
- <skill-2>: <why>

### Suggested Next Steps
- <Based on branch state and plan doc status>
```

## Step 5: Confirm Direction

After presenting the briefing, ask:

> Does this look right? What are we working on today?

Wait for human confirmation before proceeding to any code changes.

## Branch Name Convention Decoding

| Prefix | Meaning | Expected workflow |
|--------|---------|-------------------|
| `feature/` | New feature | Plan -> implement -> test -> PR |
| `bugfix/` | Bug fix | Investigate -> fix -> regression test -> PR |
| `hotfix/` | Urgent fix | Fix -> test -> PR |
| `improvement/` | Enhancement | Understand scope -> implement -> verify -> PR |
| `chore/` | Maintenance | Execute -> verify -> PR |

## Skill Suggestions by Area

| Topic keywords | Skills to suggest |
|---------------|-------------------|
| stash, graphql, query, api | `stash`, `graphql-patterns` |
| ui, settings, component, tailwind | `tailwind-css-patterns`, `frontend-design` |
| rotation, engine, timer, wallpaper | (Rust core — no specific skill, use `cargo test`) |
| tray, window, tauri | (Tauri-specific — check Tauri v2 docs) |
| build, release, deploy | `git-preferences` |

## What This Skill Does NOT Do

- Does not make decisions about what to work on (that's the human's call)
- Does not start coding (wait for confirmation)
- Does not replace `work-ticket` (which handles the full ticket lifecycle)
- Queries brain for context but does not store new memories
