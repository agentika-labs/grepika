//! Code-graph navigation tool (`graph`).
//!
//! One tool dispatches over a `relation`: callers, callees, call_chain,
//! imports, dependents. Backed by the symbols/edges tables populated during
//! indexing (see [`crate::db::graph`]). ponytail: single surface rather than
//! five near-identical handlers.

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
}

const fn default_depth() -> usize {
    5
}

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

/// Output for the graph tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GraphOutput {
    pub relation: String,
    pub name: String,
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

    let mut symbols = Vec::new();
    let mut modules = Vec::new();

    match input.relation.as_str() {
        "callers" => {
            symbols = db
                .callers(&input.name)?
                .into_iter()
                .map(|s| hit(s, true, &rel))
                .collect();
        }
        "callees" => {
            for (callee, def) in db.callees(&input.name)? {
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
                .call_chain(&input.name, input.depth)?
                .into_iter()
                .map(|s| hit(s, true, &rel))
                .collect();
        }
        "imports" => {
            // name is a file path relative to root; match against stored path.
            let abs = root.join(&input.name);
            modules = db.imports_of(&abs.to_string_lossy())?;
            if modules.is_empty() {
                modules = db.imports_of(&input.name)?;
            }
        }
        "dependents" => {
            modules = db
                .dependents_of(&input.name)?
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

    Ok(GraphOutput {
        relation: input.relation,
        name: input.name,
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

fn relativize(p: &str, root: &Path) -> String {
    Path::new(p)
        .strip_prefix(root)
        .map(|r| r.to_string_lossy().to_string())
        .unwrap_or_else(|_| p.to_string())
}
