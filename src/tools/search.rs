//! Search-related MCP tools.

use crate::security;
use crate::services::SearchService;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Relativizes a path against the workspace root.
fn relativize_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

/// Maps internal `MatchSnippet` to the output representation.
fn map_snippets(snippets: &[crate::services::MatchSnippet]) -> Vec<MatchSnippetOutput> {
    snippets
        .iter()
        .map(|s| MatchSnippetOutput {
            line: s.line_number,
            text: s.line_content.clone(),
            highlight_start: s.match_start,
            highlight_end: s.match_end,
        })
        .collect()
}

/// Search mode for controlling which backend(s) to use.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Use all backends with weighted score merging (best quality)
    #[default]
    Combined,
    /// FTS5 full-text search only (best for natural language)
    Fts,
    /// Grep regex search only (best for patterns)
    Grep,
}

impl std::fmt::Display for SearchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Combined => write!(f, "combined"),
            Self::Fts => write!(f, "fts"),
            Self::Grep => write!(f, "grep"),
        }
    }
}

impl std::str::FromStr for SearchMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "combined" => Ok(Self::Combined),
            "fts" => Ok(Self::Fts),
            "grep" => Ok(Self::Grep),
            other => Err(format!(
                "Invalid search mode: '{}'. Valid modes: combined, fts, grep",
                other
            )),
        }
    }
}

/// Input for the search tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchInput {
    /// Search query (supports regex)
    pub query: String,
    /// Maximum results to return (default: 20)
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Search mode: "combined", "fts", or "grep"
    #[serde(default)]
    pub mode: SearchMode,
}

const fn default_limit() -> usize {
    20
}

/// A matching snippet showing where a result matched.
#[derive(Debug, Serialize, JsonSchema)]
pub struct MatchSnippetOutput {
    /// Line number.
    #[serde(rename = "l")]
    pub line: u64,
    /// Matching line text.
    #[serde(rename = "t")]
    pub text: String,
    /// Match start byte offset.
    #[serde(rename = "hs")]
    #[serde(skip_serializing_if = "is_zero")]
    pub highlight_start: usize,
    /// Match end byte offset.
    #[serde(rename = "he")]
    #[serde(skip_serializing_if = "is_zero")]
    pub highlight_end: usize,
}

const fn is_zero(v: &usize) -> bool {
    *v == 0
}

const fn is_false(v: &bool) -> bool {
    !*v
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Output for the search tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchOutput {
    /// Ranked results.
    #[serde(rename = "r")]
    pub results: Vec<SearchResultItem>,
    /// True when more results exist.
    #[serde(rename = "more")]
    #[serde(skip_serializing_if = "is_false")]
    pub has_more: bool,
    /// Guidance for empty/incomplete results.
    #[serde(rename = "hint")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// A single search result.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchResultItem {
    /// File path relative to root.
    #[serde(rename = "p")]
    pub path: String,
    /// Relevance score.
    #[serde(rename = "s")]
    pub score: f64,
    /// Match sources: f=fts, g=grep, t=trigram.
    #[serde(rename = "src")]
    pub sources: String,
    /// Matching line snippets.
    #[serde(rename = "m")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub snippets: Vec<MatchSnippetOutput>,
}

/// Executes the search tool.
///
/// # Errors
///
/// Returns a `ServerError` if the search operation fails.
pub fn execute_search(
    service: &SearchService,
    input: SearchInput,
) -> crate::error::Result<SearchOutput> {
    // Check for empty index before searching (uses cached AtomicU64, no DB round-trip)
    if service.cached_total_files() == 0 {
        return Err(crate::error::ServerError::Tool(
            "Index is empty. Run the 'index' tool first to build the search index, then retry your search."
                .into(),
        ));
    }

    // Overcollect by 1 to detect if more results exist
    let request_limit = input.limit.saturating_add(1);

    let results = match input.mode {
        SearchMode::Fts => service.search_fts(&input.query, request_limit)?,
        SearchMode::Grep => service.search_grep(&input.query, request_limit)?,
        SearchMode::Combined => service.search(&input.query, request_limit)?,
    };

    let has_more = results.len() > input.limit;
    let root = service.root();
    let items: Vec<_> = results
        .iter()
        .take(input.limit)
        .filter(|r| security::is_sensitive_file(&r.path).is_none())
        .map(|r| SearchResultItem {
            path: relativize_path(&r.path, root),
            score: round2(r.score.as_f64()),
            sources: r.sources.to_compact(),
            snippets: map_snippets(&r.snippets),
        })
        .collect();

    let hint = if items.is_empty() {
        let suggestion = match input.mode {
            SearchMode::Grep => "Try mode=fts for natural language or mode=combined for broader matching.",
            SearchMode::Fts => "Try mode=grep for exact regex or mode=combined for broader matching.",
            SearchMode::Combined => "Try a broader query, different keywords, or check 'stats' to verify index coverage.",
        };
        Some(format!("No results found. {suggestion}"))
    } else {
        None
    };

    Ok(SearchOutput {
        results: items,
        has_more,
        hint,
    })
}
