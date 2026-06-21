#!/usr/bin/env bash
# Grepika awareness hook — reminds Claude to use grepika MCP tools over built-in search
cat <<'EOF'
GREPIKA AVAILABLE: For code search, ALWAYS use mcp__grepika__search (ranked results, ~6x smaller responses) instead of Grep. For file discovery, use mcp__grepika__toc instead of Glob. For symbol tracing, use mcp__grepika__refs instead of grepping for identifiers. Use mcp__grepika__structural_search for AST patterns and mcp__grepika__graph for indexed call/import navigation. If workspace not loaded, call mcp__grepika__add_workspace first; run /index before indexed search or graph navigation.
EOF
