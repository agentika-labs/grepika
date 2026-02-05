//! Search-related MCP tools.

use crate::services::SearchService;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Input for the search tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchInput {
    /// Search query (supports regex)
    pub query: String,
    /// Maximum results to return (default: 20)
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Search mode: "combined", "fts", or "grep"
    #[serde(default = "default_mode")]
    pub mode: String,
}

fn default_limit() -> usize {
    20
}

fn default_mode() -> String {
    "combined".to_string()
}

/// Output for the search tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchOutput {
    /// Search results
    pub results: Vec<SearchResultItem>,
    /// Total matches found
    pub total: usize,
    /// Query that was executed
    pub query: String,
}

/// A single search result.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchResultItem {
    /// File path relative to root
    pub path: String,
    /// Relevance score (0.0 - 1.0)
    pub score: f64,
    /// Search sources that matched
    pub sources: Vec<String>,
}

/// Executes the search tool.
///
/// # Errors
///
/// Returns an error string if the search operation fails.
pub fn execute_search(
    service: &Arc<SearchService>,
    input: SearchInput,
) -> Result<SearchOutput, String> {
    let results = match input.mode.as_str() {
        "fts" => service
            .search_fts(&input.query, input.limit)
            .map_err(|e| e.to_string())?,
        "grep" => service
            .search_grep(&input.query, input.limit)
            .map_err(|e| e.to_string())?,
        _ => service
            .search(&input.query, input.limit)
            .map_err(|e| e.to_string())?,
    };

    let root = service.root();
    let items: Vec<_> = results
        .iter()
        .map(|r| {
            let relative_path = r
                .path
                .strip_prefix(root)
                .unwrap_or(&r.path)
                .to_string_lossy()
                .to_string();

            let mut sources = Vec::new();
            if r.sources.fts {
                sources.push("fts".to_string());
            }
            if r.sources.grep {
                sources.push("grep".to_string());
            }
            if r.sources.trigram {
                sources.push("trigram".to_string());
            }

            SearchResultItem {
                path: relative_path,
                score: r.score.as_f64(),
                sources,
            }
        })
        .collect();

    Ok(SearchOutput {
        total: items.len(),
        results: items,
        query: input.query,
    })
}

/// Input for the relevant tool (finds most relevant files for a topic).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RelevantInput {
    /// Topic or concept to find relevant files for
    pub topic: String,
    /// Maximum files to return (default: 10)
    #[serde(default = "default_relevant_limit")]
    pub limit: usize,
}

fn default_relevant_limit() -> usize {
    10
}

/// Output for the relevant tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RelevantOutput {
    /// Most relevant files
    pub files: Vec<RelevantFile>,
    /// Topic searched
    pub topic: String,
}

/// A relevant file result.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RelevantFile {
    /// File path
    pub path: String,
    /// Relevance score
    pub score: f64,
    /// Brief explanation of relevance
    pub reason: String,
}

/// Executes the relevant tool.
///
/// # Errors
///
/// Returns an error string if the search operation fails.
pub fn execute_relevant(
    service: &Arc<SearchService>,
    input: RelevantInput,
) -> Result<RelevantOutput, String> {
    // Use combined search for best relevance
    let results = service
        .search(&input.topic, input.limit)
        .map_err(|e| e.to_string())?;

    let root = service.root();
    let files: Vec<_> = results
        .iter()
        .map(|r| {
            let relative_path = r
                .path
                .strip_prefix(root)
                .unwrap_or(&r.path)
                .to_string_lossy()
                .to_string();

            // Generate reason based on sources
            let reason = match (r.sources.fts, r.sources.grep, r.sources.trigram) {
                (true, true, true) => "Strong match: content, pattern, and substring".to_string(),
                (true, true, false) => "Good match: content and pattern".to_string(),
                (true, false, true) => "Match: content and substring".to_string(),
                (false, true, true) => "Match: pattern and substring".to_string(),
                (true, false, false) => "Content match".to_string(),
                (false, true, false) => "Pattern match".to_string(),
                (false, false, true) => "Substring match".to_string(),
                (false, false, false) => "Unknown".to_string(),
            };

            RelevantFile {
                path: relative_path,
                score: r.score.as_f64(),
                reason,
            }
        })
        .collect();

    Ok(RelevantOutput {
        files,
        topic: input.topic,
    })
}
