//! Code-graph navigation tool (`graph`).
//!
//! One tool dispatches over a `relation`: callers, callees, call_chain,
//! imports, dependents. Backed by the symbols/edges tables populated during
//! indexing (see [`crate::db::graph`]).

use crate::security;
use crate::services::SearchService;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// Input for the graph tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GraphInput {
    /// Relation to query: callers | callees | call_chain | imports | dependents
    pub relation: String,
    /// Symbol name (callers/callees/call_chain) or file path (imports) or
    /// module substring (dependents).
    pub name: String,
    /// Max depth for call_chain (default 5).
    #[serde(default = "default_depth")]
    pub depth: usize,
    /// Maximum results to return (default 100).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

const fn default_depth() -> usize {
    5
}

const fn default_limit() -> usize {
    100
}

const MAX_LIMIT: usize = 500;
pub const MAX_DEPTH: usize = 25;

/// A symbol result row.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SymbolHit {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    /// For `callees`: whether the callee resolves to a known definition.
    #[serde(skip_serializing_if = "is_true")]
    pub resolved: bool,
}

const fn is_true(b: &bool) -> bool {
    *b
}

const fn is_false(b: &bool) -> bool {
    !*b
}

/// Output for the graph tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GraphOutput {
    pub relation: String,
    pub name: String,
    /// Whether output was truncated to the requested limit.
    #[serde(skip_serializing_if = "is_false")]
    pub truncated: bool,
    /// Symbol results (callers / callees / call_chain).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<SymbolHit>,
    /// Module/path results (imports / dependents).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
}

/// Executes the graph tool.
///
/// # Errors
///
/// Returns `ServerError` for an unknown relation or a database failure.
pub fn execute_graph(
    service: &Arc<SearchService>,
    input: GraphInput,
) -> crate::error::Result<GraphOutput> {
    let db = service.db();
    let root = service.root();
    let rel = |p: String| relativize(&p, root);
    let limit = if input.limit == 0 {
        default_limit()
    } else {
        input.limit.min(MAX_LIMIT)
    };
    let depth = if input.depth == 0 {
        default_depth()
    } else {
        input.depth.min(MAX_DEPTH)
    };
    let fetch_limit = limit.saturating_add(1);

    if input.name.trim().is_empty() {
        return Err(crate::error::ServerError::Tool(
            "graph name must not be empty".to_string(),
        ));
    }

    let mut symbols = Vec::new();
    let mut modules = Vec::new();

    match input.relation.as_str() {
        "callers" => {
            symbols = db
                .callers_limited(&input.name, fetch_limit)?
                .into_iter()
                .map(|s| hit(s, true, &rel))
                .collect();
        }
        "callees" => {
            for (callee, def) in db.callees_limited(&input.name, fetch_limit)? {
                match def {
                    Some(s) => symbols.push(hit(s, true, &rel)),
                    None => symbols.push(SymbolHit {
                        name: callee,
                        kind: "unresolved".to_string(),
                        path: String::new(),
                        start_line: 0,
                        end_line: 0,
                        resolved: false,
                    }),
                }
            }
        }
        "call_chain" => {
            symbols = db
                .call_chain_limited(&input.name, depth, fetch_limit)?
                .into_iter()
                .map(|s| hit(s, true, &rel))
                .collect();
        }
        "imports" => {
            // name is a file path relative to root; validate the same path
            // contract as read-oriented tools even though this only queries DB.
            let validated = security::validate_read_access(root, &input.name)?;
            let canonical_root = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
            let joined = root.join(&input.name);
            let mut candidates = Vec::with_capacity(5);
            push_unique(&mut candidates, joined.to_string_lossy().into_owned());
            push_unique(&mut candidates, validated.to_string_lossy().into_owned());
            if let Ok(relative) = validated.strip_prefix(root) {
                push_unique(
                    &mut candidates,
                    canonical_root.join(relative).to_string_lossy().into_owned(),
                );
                push_unique(&mut candidates, relative.to_string_lossy().into_owned());
            }
            if let Ok(relative) = validated.strip_prefix(&canonical_root) {
                push_unique(
                    &mut candidates,
                    root.join(relative).to_string_lossy().into_owned(),
                );
                push_unique(&mut candidates, relative.to_string_lossy().into_owned());
            }
            push_unique(&mut candidates, input.name.clone());

            for candidate in candidates {
                modules = db.imports_of_limited(&candidate, fetch_limit)?;
                if !modules.is_empty() {
                    break;
                }
            }
        }
        "dependents" => {
            modules = db
                .dependents_of_limited(&input.name, fetch_limit)?
                .into_iter()
                .map(rel)
                .collect();
        }
        other => {
            return Err(crate::error::ServerError::Tool(format!(
                "unknown relation '{other}'; expected callers|callees|call_chain|imports|dependents"
            )));
        }
    }

    let truncated = symbols.len() > limit || modules.len() > limit;
    symbols.truncate(limit);
    modules.truncate(limit);

    Ok(GraphOutput {
        relation: input.relation,
        name: input.name,
        truncated,
        symbols,
        modules,
    })
}

fn hit(s: crate::db::GraphSymbol, resolved: bool, rel: &impl Fn(String) -> String) -> SymbolHit {
    SymbolHit {
        name: s.name,
        kind: s.kind,
        path: rel(s.path),
        start_line: s.start_line,
        end_line: s.end_line,
        resolved,
    }
}

fn push_unique(candidates: &mut Vec<String>, value: String) {
    if !value.is_empty() && !candidates.iter().any(|candidate| candidate == &value) {
        candidates.push(value);
    }
}

fn relativize(p: &str, root: &Path) -> String {
    Path::new(p)
        .strip_prefix(root)
        .map(|r| r.to_string_lossy().to_string())
        .unwrap_or_else(|_| p.to_string())
}
