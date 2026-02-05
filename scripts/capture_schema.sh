#!/bin/bash
# Capture MCP schema JSON for token cost estimation.
#
# This script generates the tool schema programmatically since
# the MCP server requires proper JSON-RPC initialization sequence.
#
# Usage:
#   ./scripts/capture_schema.sh
#
# Output:
#   - schema.json: The tools schema
#   - Token count estimation

set -e

# Build the server if needed
if [ ! -f "target/release/agentika-grep" ]; then
    echo "Building agentika-grep..."
    cargo build --release
fi

# Generate schema using the benchmark utility's schema
# This matches what rmcp generates for the tools/list response
cat > schema.json << 'SCHEMA_EOF'
{
  "tools": [
    {
      "name": "search",
      "description": "Search for code patterns. Supports regex and natural language queries.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "query": {"type": "string", "description": "Search query (regex or natural language)"},
          "limit": {"type": "integer", "description": "Maximum results (default: 20)"},
          "mode": {"type": "string", "description": "Search mode: combined, fts, or grep (default: combined)"}
        },
        "required": ["query"]
      }
    },
    {
      "name": "relevant",
      "description": "Find files most relevant to a topic. Uses combined search for best results.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "topic": {"type": "string", "description": "Topic or concept to search for"},
          "limit": {"type": "integer", "description": "Maximum files (default: 10)"}
        },
        "required": ["topic"]
      }
    },
    {
      "name": "get",
      "description": "Get file content. Supports line range selection.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "path": {"type": "string", "description": "File path relative to root"},
          "start_line": {"type": "integer", "description": "Starting line (1-indexed, default: 1)"},
          "end_line": {"type": "integer", "description": "Ending line (0 = end of file)"}
        },
        "required": ["path"]
      }
    },
    {
      "name": "outline",
      "description": "Extract file structure (functions, classes, structs, etc.)",
      "inputSchema": {
        "type": "object",
        "properties": {
          "path": {"type": "string", "description": "File path relative to root"}
        },
        "required": ["path"]
      }
    },
    {
      "name": "toc",
      "description": "Get directory tree structure",
      "inputSchema": {
        "type": "object",
        "properties": {
          "path": {"type": "string", "description": "Directory path (default: root)"},
          "depth": {"type": "integer", "description": "Maximum depth (default: 3)"}
        }
      }
    },
    {
      "name": "context",
      "description": "Get surrounding context for a line",
      "inputSchema": {
        "type": "object",
        "properties": {
          "path": {"type": "string", "description": "File path"},
          "line": {"type": "integer", "description": "Center line number"},
          "context_lines": {"type": "integer", "description": "Lines of context before and after (default: 10)"}
        },
        "required": ["path", "line"]
      }
    },
    {
      "name": "stats",
      "description": "Get index statistics and file type breakdown",
      "inputSchema": {
        "type": "object",
        "properties": {
          "detailed": {"type": "boolean", "description": "Include detailed breakdown by file type"}
        }
      }
    },
    {
      "name": "related",
      "description": "Find files related to a source file by shared symbols",
      "inputSchema": {
        "type": "object",
        "properties": {
          "path": {"type": "string", "description": "Source file path"},
          "limit": {"type": "integer", "description": "Maximum related files (default: 10)"}
        },
        "required": ["path"]
      }
    },
    {
      "name": "refs",
      "description": "Find all references to a symbol/identifier",
      "inputSchema": {
        "type": "object",
        "properties": {
          "symbol": {"type": "string", "description": "Symbol/identifier to find"},
          "limit": {"type": "integer", "description": "Maximum references (default: 50)"}
        },
        "required": ["symbol"]
      }
    },
    {
      "name": "index",
      "description": "Update the search index (incremental by default)",
      "inputSchema": {
        "type": "object",
        "properties": {
          "force": {"type": "boolean", "description": "Force full re-index"}
        }
      }
    },
    {
      "name": "diff",
      "description": "Show differences between two files",
      "inputSchema": {
        "type": "object",
        "properties": {
          "file1": {"type": "string", "description": "First file path"},
          "file2": {"type": "string", "description": "Second file path"},
          "context": {"type": "integer", "description": "Context lines around changes (default: 3)"}
        },
        "required": ["file1", "file2"]
      }
    }
  ]
}
SCHEMA_EOF

# Pretty print if jq is available
if command -v jq &> /dev/null; then
    jq '.' schema.json > schema.json.tmp && mv schema.json.tmp schema.json
fi

# Calculate token estimate
BYTES=$(wc -c < schema.json | tr -d ' ')
TOKENS=$((BYTES / 4))

echo ""
echo "═══════════════════════════════════════════════════"
echo "           MCP SCHEMA TOKEN ANALYSIS               "
echo "═══════════════════════════════════════════════════"
echo ""
echo "Schema size:      $BYTES bytes"
echo "Estimated tokens: $TOKENS tokens"
echo ""
echo "Schema saved to: schema.json"

# Show tool count if jq is available
if command -v jq &> /dev/null; then
    TOOL_COUNT=$(jq '.tools | length' schema.json 2>/dev/null || echo "?")
    echo "Tools defined:    $TOOL_COUNT"
fi

echo ""
echo "This one-time cost is paid when the MCP server initializes."
echo "Per-query savings offset this cost after N queries."
echo ""
echo "Run 'cargo bench token_efficiency' for break-even analysis."
