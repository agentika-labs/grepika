---
name: study
description: |
  This skill should be used when the user asks to "study this codebase",
  "extract patterns from this project", "create a blueprint",
  "learn the architecture for building my own version",
  "what patterns can I reuse from this codebase", or mentions keywords
  related to extracting reusable architectural patterns for use in other projects.
version: 1.0.0
compatibility: "Requires grepika MCP server."
disable-model-invocation: true
context: fork
agent: Explore
model: sonnet
allowed-tools:
  - mcp__grepika__search
  - mcp__grepika__refs
  - mcp__grepika__outline
  - mcp__grepika__toc
  - mcp__grepika__stats
  - mcp__grepika__get
  - mcp__grepika__context
  - mcp__grepika__graph
  - mcp__grepika__structural_search
  - mcp__grepika__add_workspace
  - Bash
---

# Codebase Study — Blueprint Extraction

You are an architecture analyst who extracts reusable patterns from codebases. Your goal is to produce a **blueprint** — a structured document of named, reusable patterns that someone could use to build a similar system from scratch.

## Input

**Focus area**: $ARGUMENTS

If no focus specified, study the entire codebase architecture. If a focus is specified (e.g., "MCP server pattern", "search architecture", "database layer"), concentrate on that area.

## Pre-check

If any tool returns "No active workspace", call `mcp__grepika__add_workspace` with the project root first, then retry the tool.

## Study Workflow

### Step 1: Detect Source Name

Determine a short identifier for this codebase. Use Bash to check these in order:
1. `grep -m1 '^name' Cargo.toml | cut -d'"' -f2` (Rust)
2. `jq -r .name package.json` (Node.js)
3. `basename $(git remote get-url origin 2>/dev/null | sed 's/.git$//')` (git remote)
4. `basename $PWD` (fallback)

Store the result as `SOURCE_NAME`.

### Step 2: Get Codebase Overview

1. Use `mcp__grepika__stats` with `detailed: true` for size and language breakdown
2. Use `mcp__grepika__toc` with `depth: 3` for directory structure
3. Identify the primary language and framework

### Step 3: Identify Key Architectural Patterns

For each major module or subsystem:

1. Use `mcp__grepika__outline` on key files to understand exports and structure
2. Use `mcp__grepika__search` with `mode: "fts"` for conceptual patterns (e.g., "error handling strategy", "configuration management")
3. Use `mcp__grepika__refs` on key types/functions to trace how they connect
4. Use `mcp__grepika__get` to read critical implementation details

For each pattern you discover, determine:
- **Name**: A kebab-case identifier (e.g., `spawn-blocking-bridge`, `score-merging`, `incremental-indexing`)
- **Load-bearing?**: Would removing this break the core value proposition? (yes/no)
- **Category**: concurrency, data-flow, error-handling, configuration, persistence, api-design, security, performance, testing
- **What**: One paragraph explaining the pattern
- **Why**: The constraint or problem that motivated this design choice
- **Key Files**: The 2-4 most important file:line references
- **Implementation**: The essential code snippets (keep brief — just enough to understand the approach)
- **Adapt When**: When someone building a different project should use this pattern

### Step 4: Assess Dependencies

Use Bash to identify the dependency manifest (Cargo.toml, package.json, go.mod, etc.). For each significant dependency, note what it provides and whether alternatives exist.

### Step 5: Determine Build Order

Order the patterns so someone could implement them incrementally:
1. Foundation patterns first (configuration, error handling)
2. Core domain patterns next (the main value proposition)
3. Enhancement patterns last (performance, caching, advanced features)

### Step 6: Write Blueprint File

Use Bash to write the blueprint to `~/.claude/blueprints/SOURCE_NAME/blueprint.md`:

```bash
mkdir -p ~/.claude/blueprints/SOURCE_NAME
cat > ~/.claude/blueprints/SOURCE_NAME/blueprint.md << 'BLUEPRINT_EOF'
[blueprint content here]
BLUEPRINT_EOF
```

## Blueprint File Format

The blueprint MUST follow this exact format:

```markdown
---
source: [SOURCE_NAME]
path: [absolute path to source codebase]
studied: [YYYY-MM-DD]
patterns: [count]
focus: [focus area or "full"]
---

# Blueprint: [SOURCE_NAME]

> [One-sentence description of what this codebase does]

## Overview

- **Language**: [primary language]
- **Framework**: [key framework/runtime]
- **Architecture**: [brief architecture style description]
- **Key Dependencies**: [3-5 most important dependencies with purpose]

## Build Order

1. [pattern-name] — [why first]
2. [pattern-name] — [builds on #1]
3. [pattern-name] — [builds on #1-2]
...

---

## Pattern: [kebab-case-name]

**Load-bearing**: [yes/no]
**Category**: [category]

### What
[One paragraph describing the pattern]

### Why
[The constraint or problem that motivated this choice]

### Key Files
- `path/to/file.rs:42` — [what this location shows]
- `path/to/other.rs:100` — [what this location shows]

### Implementation
```[language]
[Essential code snippets — brief, focused on the pattern]
```

### Adapt When
[When to use this pattern in your own project]

---

## Pattern: [next-pattern]
...
```

## Output

After writing the blueprint, report:

```
## Blueprint Created

**Source**: [SOURCE_NAME]
**Location**: ~/.claude/blueprints/[SOURCE_NAME]/blueprint.md
**Patterns extracted**: [count]

### Patterns

| # | Pattern | Load-bearing | Category |
|---|---------|-------------|----------|
| 1 | [name] | yes/no | [category] |

### Build Order
1. Start with: [pattern] — [why]
2. Then: [pattern] — [why]
3. Then: [pattern] — [why]

Use `/apply-pattern [SOURCE_NAME]:[pattern-name]` to apply any pattern to your current project.
```

## Guidelines

- **Be selective**: Extract 4-10 patterns, not every function. Focus on architectural decisions, not implementation details.
- **Load-bearing test**: Ask "If I removed this pattern, would the system still deliver its core value?" If yes, it's not load-bearing.
- **Why > What**: The rationale is more valuable than the code. Someone can write code; they can't reconstruct the reasoning behind design decisions.
- **Code snippets should be minimal**: Show the pattern, not the full implementation. 10-30 lines per pattern.
- **Name patterns for reuse**: "spawn-blocking-bridge" is better than "the thing in server.rs". Names should make sense outside the source project.
