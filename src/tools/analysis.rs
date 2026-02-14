//! Analysis-related MCP tools.
//!
//! # Security
//!
//! The `refs` tool validates paths to prevent traversal
//! attacks and blocks access to sensitive files.
//!
//! See [`crate::security`] for details.

use crate::security;
use crate::services::{Indexer, SearchService};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Classification of how a symbol is used at a reference site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Definition,
    Import,
    TypeUsage,
    Usage,
}

impl fmt::Display for RefKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Definition => f.write_str("definition"),
            Self::Import => f.write_str("import"),
            Self::TypeUsage => f.write_str("type_usage"),
            Self::Usage => f.write_str("usage"),
        }
    }
}

/// Input for the stats tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct StatsInput {
    /// Whether to include detailed breakdown
    #[serde(default)]
    pub detailed: bool,
}

/// Output for the stats tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StatsOutput {
    /// Total indexed files
    pub total_files: u64,
    /// Total trigrams indexed
    pub trigram_count: usize,
    /// Breakdown by file type (if detailed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_type: Option<HashMap<String, u64>>,
    /// Index size info
    pub index_size: IndexSize,
}

/// Index size information.
#[derive(Debug, Serialize, JsonSchema)]
pub struct IndexSize {
    /// Approximate index size in bytes
    pub bytes: u64,
    /// Human-readable size
    pub human: String,
}

/// Executes the stats tool.
///
/// # Errors
///
/// Returns a `ServerError` if statistics retrieval fails.
pub fn execute_stats(
    service: &Arc<SearchService>,
    indexer: &Indexer,
    input: StatsInput,
) -> crate::error::Result<StatsOutput> {
    let stats = indexer.stats()?;

    let by_type = if input.detailed {
        let indexed_files = service.db().get_indexed_files()?;
        let mut counts: HashMap<String, u64> = HashMap::new();

        for (path, _) in indexed_files {
            let ext = std::path::Path::new(&path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("unknown")
                .to_lowercase();
            *counts.entry(ext).or_insert(0) += 1;
        }

        Some(counts)
    } else {
        None
    };

    // Estimate index size (rough approximation)
    let bytes = stats.file_count * 1000 + stats.trigram_count as u64 * 20;
    let human = format_bytes(bytes);

    Ok(StatsOutput {
        total_files: stats.file_count,
        trigram_count: stats.trigram_count,
        by_type,
        index_size: IndexSize { bytes, human },
    })
}

/// Input for the refs tool (find references).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RefsInput {
    /// Symbol/identifier to find references for
    pub symbol: String,
    /// Maximum references to return
    #[serde(default = "default_refs_limit")]
    pub limit: usize,
}

const fn default_refs_limit() -> usize {
    50
}

/// Output for the refs tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RefsOutput {
    /// References found
    pub references: Vec<Reference>,
}

/// A reference to a symbol.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Reference {
    /// File path
    pub path: String,
    /// Line number
    pub line: usize,
    /// Line content
    pub content: String,
    /// Reference type (definition, usage, import)
    pub ref_type: String,
}

/// Executes the refs tool.
///
/// # Security
///
/// - Search results are filtered to exclude sensitive files
/// - Grep already constrains search to the root directory
///
/// # Errors
///
/// Returns a `ServerError` if the grep search fails.
pub fn execute_refs(
    service: &Arc<SearchService>,
    input: RefsInput,
) -> crate::error::Result<RefsOutput> {
    // Use grep to find exact symbol matches, keeping raw GrepMatch data
    // to avoid re-reading files (the old approach doubled I/O).
    let matches_by_file = service.search_grep_with_matches(
        &format!(r"\b{}\b", regex::escape(&input.symbol)),
        input.limit * 2,
    )?;

    let root = service.root();
    let mut references = Vec::new();

    for (path, matches) in &matches_by_file {
        // Security: skip sensitive files from search results
        if security::is_sensitive_file(path).is_some() {
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        for m in matches {
            let trimmed = m.line_content.trim();
            let ref_type = classify_reference(trimmed, &input.symbol);

            references.push(Reference {
                path: relative.clone(),
                line: m.line_number as usize,
                content: trim_around_match(trimmed, &input.symbol),
                ref_type: ref_type.to_string(),
            });

            if references.len() >= input.limit {
                break;
            }
        }

        if references.len() >= input.limit {
            break;
        }
    }

    Ok(RefsOutput { references })
}

// Helper functions

/// Trims a line to ~60 chars centered on the first occurrence of `symbol`.
/// If the line is short enough, returns it unchanged.
fn trim_around_match(line: &str, symbol: &str) -> String {
    const MAX_LEN: usize = 60;
    if line.len() <= MAX_LEN {
        return line.to_string();
    }
    let match_pos = match line.find(symbol) {
        Some(pos) => pos,
        None => return line[..line.floor_char_boundary(MAX_LEN)].to_string(),
    };
    // Center a window around the match
    let window_start = match_pos.saturating_sub((MAX_LEN - symbol.len()) / 2);
    let window_end = (window_start + MAX_LEN).min(line.len());
    // Snap to char boundaries
    let safe_start = line.ceil_char_boundary(window_start);
    let safe_end = line.floor_char_boundary(window_end);
    let mut result = String::with_capacity(MAX_LEN + 6);
    if safe_start > 0 {
        result.push_str("...");
    }
    result.push_str(&line[safe_start..safe_end]);
    if safe_end < line.len() {
        result.push_str("...");
    }
    result
}

/// Classifies a reference line as definition/import/type_usage/usage.
fn classify_reference(line: &str, symbol: &str) -> RefKind {
    let trimmed = line.trim();

    // Check for definitions
    let is_definition_keyword = trimmed.starts_with("fn ")
        || trimmed.starts_with("pub fn ")
        || trimmed.starts_with("def ")
        || trimmed.starts_with("function ")
        || trimmed.starts_with("class ")
        || trimmed.starts_with("struct ")
        || trimmed.starts_with("enum ")
        || trimmed.starts_with("type ")
        || trimmed.starts_with("interface ");

    if is_definition_keyword
        && (trimmed.contains(&format!(" {symbol}"))
            || trimmed.contains(&format!(" {symbol}("))
            || trimmed.contains(&format!(" {symbol}<")))
    {
        return RefKind::Definition;
    }

    // Check for imports
    if trimmed.starts_with("use ")
        || trimmed.starts_with("import ")
        || trimmed.starts_with("from ")
        || trimmed.contains("require(")
    {
        return RefKind::Import;
    }

    // Check for type annotations
    if trimmed.contains(&format!(": {symbol}"))
        || trimmed.contains(&format!("-> {symbol}"))
        || trimmed.contains(&format!("<{symbol}"))
    {
        return RefKind::TypeUsage;
    }

    RefKind::Usage
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
