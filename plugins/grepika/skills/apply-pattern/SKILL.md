---
name: apply-pattern
description: |
  This skill should be used when the user asks to "apply a pattern",
  "use a blueprint", "implement [pattern] from [source]",
  "what blueprints do I have", "list patterns", "adapt this pattern",
  or mentions keywords related to applying previously extracted
  architectural patterns to their current project.
version: 1.0.0
compatibility: "Requires grepika MCP server."
disable-model-invocation: true
context: fork
agent: Explore
model: haiku
allowed-tools:
  - mcp__grepika__search
  - mcp__grepika__refs
  - mcp__grepika__outline
  - mcp__grepika__toc
  - mcp__grepika__get
  - mcp__grepika__structural_search
  - mcp__grepika__add_workspace
  - Bash
  - Read
---

# Apply Pattern — Blueprint-Guided Implementation

You help developers apply architectural patterns from previously studied codebases to their current project.

## Input

**Arguments**: $ARGUMENTS

## Argument Parsing

Determine the action from the arguments:

1. **`--list`** → List all available blueprints
2. **`<source>`** (single word, no colon) → List patterns in that blueprint
3. **`<source>:<pattern>`** → Apply a specific pattern to the current project
4. **`<pattern>`** (contains hyphen, no colon) → Search all blueprints for this pattern name

If no arguments provided, run `--list`.

## Pre-check

If any grepika tool returns "No active workspace", call `mcp__grepika__add_workspace` with the project root first.

## Action: List Blueprints (`--list`)

Use Bash to list available blueprints:

```bash
ls -1 ~/.claude/blueprints/ 2>/dev/null || echo "No blueprints found. Use /study to extract patterns from a codebase first."
```

For each blueprint found, use Bash to extract the frontmatter:

```bash
head -10 ~/.claude/blueprints/*/blueprint.md
```

Output:

```
## Available Blueprints

| Source | Patterns | Studied | Focus |
|--------|----------|---------|-------|
| [name] | [count] | [date] | [focus] |

Use `/apply-pattern <source>` to see patterns, or `/apply-pattern <source>:<pattern>` to apply one.
```

## Action: List Patterns (`<source>`)

Use Read to read `~/.claude/blueprints/<source>/blueprint.md`.

Extract all `## Pattern: <name>` headings and their **Load-bearing** and **Category** fields.

Output:

```
## Patterns in [source]

| # | Pattern | Load-bearing | Category |
|---|---------|-------------|----------|
| 1 | [name] | yes/no | [category] |

### Build Order
[from the blueprint's Build Order section]

Use `/apply-pattern [source]:[pattern-name]` to apply a pattern.
```

## Action: Apply Pattern (`<source>:<pattern>`)

This is the core workflow.

### Step 1: Read the Pattern

Use Read to read `~/.claude/blueprints/<source>/blueprint.md`.

Find the section starting with `## Pattern: <pattern>` and ending at the next `## Pattern:` or end of file. Extract:
- What / Why / Key Files / Implementation / Adapt When

If the pattern is not found, list available patterns and suggest the closest match.

### Step 2: Understand the Current Project

Use grepika tools to analyze the target project:

1. `mcp__grepika__toc` — directory structure and language
2. `mcp__grepika__search` with `mode: "fts"` — find existing code related to the pattern's domain
3. `mcp__grepika__outline` — structure of files where the pattern would be applied
4. `mcp__grepika__refs` — existing symbols that the pattern would interact with

### Step 3: Adapt the Pattern

Compare the source pattern with the target project's:
- **Language/framework**: Translate idioms (e.g., Rust `spawn_blocking` → Node.js `worker_threads`)
- **Existing code**: Identify what already exists that the pattern can build on
- **Naming conventions**: Match the target project's style
- **Dependencies**: Recommend equivalent libraries for the target's ecosystem

### Step 4: Provide Implementation Guidance

Output:

```
## Applying Pattern: [pattern-name]
> From blueprint: [source]

### Pattern Summary
[What/Why from the blueprint — brief]

### Your Project Context
- **Language**: [target language/framework]
- **Relevant existing code**: [files/symbols that relate to this pattern]
- **What you already have**: [any partial implementation that exists]
- **What's missing**: [what needs to be built]

### Adaptation Plan

#### Step 1: [first thing to implement]
**Where**: `path/to/file.ext`
**What**: [specific guidance]
**Source reference**: [what the source codebase does at this point]

#### Step 2: [next thing]
...

### Key Differences from Source
| Aspect | Source ([source]) | Your Project |
|--------|-------------------|--------------|
| [aspect] | [source approach] | [recommended approach] |

### Dependencies to Add
| Package | Purpose | Alternative |
|---------|---------|-------------|
| [name] | [why needed] | [alternative if any] |

### What to Watch For
- [pitfall or edge case from the source pattern]
- [adaptation concern specific to the target project]
```

## Action: Search Pattern (`<pattern>`)

Use Bash to search across all blueprints:

```bash
grep -rl "## Pattern: .*<pattern>" ~/.claude/blueprints/*/blueprint.md 2>/dev/null
```

List matches and which blueprint they come from.

## Guidelines

- **Don't copy — adapt**: The goal is to apply the pattern's principles, not clone the source code
- **Respect the target**: Match the target project's conventions, not the source's
- **Highlight trade-offs**: If the source made a choice that might not apply (e.g., SQLite for a system that needs Postgres), say so
- **Be specific**: Reference actual files and symbols in the target project, not hypothetical locations
- **Build order matters**: If the pattern depends on another pattern from the same blueprint, mention it
