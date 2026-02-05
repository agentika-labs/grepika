//! Search-related MCP tools.

use crate::security;
use crate::services::SearchService;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Search mode for controlling which backend(s) to use.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Use all backends with weighted score merging (best quality)
    Combined,
    /// FTS5 full-text search only (best for natural language)
    Fts,
    /// Grep regex search only (best for patterns)
    Grep,
}

impl Default for SearchMode {
    fn default() -> Self {
        Self::Combined
    }
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

fn default_limit() -> usize {
    20
}

/// A matching snippet showing where a result matched.
#[derive(Debug, Serialize, JsonSchema)]
pub struct MatchSnippetOutput {
    /// Line number (1-indexed)
    pub line: u64,
    /// Content of the matching line (trimmed)
    pub text: String,
    /// Byte offset where the match starts within the line
    #[serde(skip_serializing_if = "is_zero")]
    pub highlight_start: usize,
    /// Byte offset where the match ends within the line
    #[serde(skip_serializing_if = "is_zero")]
    pub highlight_end: usize,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

/// Output for the search tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchOutput {
    /// Search results
    pub results: Vec<SearchResultItem>,
    /// Number of results returned
    pub total_returned: usize,
    /// Whether more results exist beyond the limit
    pub has_more: bool,
    /// Query that was executed
    pub query: String,
    /// Score interpretation guide
    pub score_guide: &'static str,
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
    /// Matching line snippets (up to 3) showing why this file matched
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub snippets: Vec<MatchSnippetOutput>,
}

/// Score guide text for search results
const SCORE_GUIDE: &str = "Scores: 0.0-1.0 scale. >0.7 excellent, 0.4-0.7 good, <0.4 weak";

/// Executes the search tool.
///
/// # Errors
///
/// Returns an error string if the search operation fails.
pub fn execute_search(
    service: &Arc<SearchService>,
    input: SearchInput,
) -> Result<SearchOutput, String> {
    // Check for empty index before searching
    if let Ok(count) = service.db().file_count() {
        if count == 0 {
            return Err(
                "Index is empty. Run the 'index' tool first to build the search index, then retry your search."
                    .to_string(),
            );
        }
    }

    // Overcollect by 1 to detect if more results exist
    let request_limit = input.limit + 1;

    let results = match input.mode {
        SearchMode::Fts => service
            .search_fts(&input.query, request_limit)
            .map_err(|e| e.to_string())?,
        SearchMode::Grep => service
            .search_grep(&input.query, request_limit)
            .map_err(|e| e.to_string())?,
        SearchMode::Combined => service
            .search(&input.query, request_limit)
            .map_err(|e| e.to_string())?,
    };

    let has_more = results.len() > input.limit;
    let root = service.root();
    let items: Vec<_> = results
        .iter()
        .take(input.limit)
        .filter(|r| security::is_sensitive_file(&r.path).is_none())
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

            let snippets: Vec<MatchSnippetOutput> = r
                .snippets
                .iter()
                .map(|s| MatchSnippetOutput {
                    line: s.line_number,
                    text: s.line_content.clone(),
                    highlight_start: s.match_start,
                    highlight_end: s.match_end,
                })
                .collect();

            SearchResultItem {
                path: relative_path,
                score: r.score.as_f64(),
                sources,
                snippets,
            }
        })
        .collect();

    Ok(SearchOutput {
        total_returned: items.len(),
        has_more,
        results: items,
        query: input.query,
        score_guide: SCORE_GUIDE,
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
    /// Number of results returned
    pub total_returned: usize,
    /// Whether more results exist
    pub has_more: bool,
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
    /// Matching line snippets showing why this file is relevant
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub snippets: Vec<MatchSnippetOutput>,
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
    // Overcollect by 1 for has_more detection
    let request_limit = input.limit + 1;

    // Use combined search for best relevance
    let results = service
        .search(&input.topic, request_limit)
        .map_err(|e| e.to_string())?;

    let has_more = results.len() > input.limit;
    let root = service.root();

    // Extract query keywords for reason generation
    let query_words: Vec<&str> = input.topic.split_whitespace().collect();

    let files: Vec<_> = results
        .iter()
        .take(input.limit)
        .filter(|r| security::is_sensitive_file(&r.path).is_none())
        .map(|r| {
            let relative_path = r
                .path
                .strip_prefix(root)
                .unwrap_or(&r.path)
                .to_string_lossy()
                .to_string();

            // Generate keyword-based reason from snippets and query
            let reason = generate_relevant_reason(r, &query_words, &relative_path);

            let snippets: Vec<MatchSnippetOutput> = r
                .snippets
                .iter()
                .map(|s| MatchSnippetOutput {
                    line: s.line_number,
                    text: s.line_content.clone(),
                    highlight_start: s.match_start,
                    highlight_end: s.match_end,
                })
                .collect();

            RelevantFile {
                path: relative_path,
                score: r.score.as_f64(),
                reason,
                snippets,
            }
        })
        .collect();

    let total_returned = files.len();
    Ok(RelevantOutput {
        files,
        topic: input.topic,
        total_returned,
        has_more,
    })
}

/// Generates a human-readable relevance reason based on matched keywords.
fn generate_relevant_reason(
    result: &crate::services::SearchHit,
    query_words: &[&str],
    path: &str,
) -> String {
    // Check which query words appear in the path or snippets
    let mut matched_keywords: Vec<String> = Vec::new();

    for &word in query_words {
        if word.len() < 3 {
            continue;
        }
        let word_lower = word.to_lowercase();

        // Check path
        if path.to_lowercase().contains(&word_lower) {
            if !matched_keywords.contains(&word.to_string()) {
                matched_keywords.push(word.to_string());
            }
            continue;
        }

        // Check snippets
        for snippet in &result.snippets {
            if snippet.line_content.to_lowercase().contains(&word_lower)
                && !matched_keywords.contains(&word.to_string())
            {
                matched_keywords.push(word.to_string());
                break;
            }
        }
    }

    if !matched_keywords.is_empty() {
        format!("Matches keywords: {}", matched_keywords.join(", "))
    } else {
        // Fallback to source-based reason
        let source_count =
            result.sources.fts as u8 + result.sources.grep as u8 + result.sources.trigram as u8;
        match source_count {
            3 => "Strong match across all search backends".to_string(),
            2 => "Good match across multiple search backends".to_string(),
            _ => "Match found".to_string(),
        }
    }
}
