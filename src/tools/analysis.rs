//! Analysis-related MCP tools.
//!
//! # Security
//!
//! The `related` and `refs` tools validate paths to prevent traversal
//! attacks and block access to sensitive files.
//!
//! See [`crate::security`] for details.

use crate::security;
use crate::services::{Indexer, SearchService};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;

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
/// Returns an error string if statistics retrieval fails.
pub fn execute_stats(
    service: &Arc<SearchService>,
    indexer: &Indexer,
    input: StatsInput,
) -> Result<StatsOutput, String> {
    let stats = indexer.stats().map_err(|e| e.to_string())?;

    let by_type = if input.detailed {
        let indexed_files = service
            .db()
            .get_indexed_files()
            .map_err(|e| e.to_string())?;
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

/// Input for the related tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RelatedInput {
    /// File path to find related files for
    pub path: String,
    /// Maximum related files to return
    #[serde(default = "default_related_limit")]
    pub limit: usize,
}

fn default_related_limit() -> usize {
    10
}

/// Output for the related tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RelatedOutput {
    /// Source file
    pub source: String,
    /// Related files
    pub related: Vec<RelatedFile>,
}

/// A related file.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RelatedFile {
    /// File path
    pub path: String,
    /// Relationship type
    pub relationship: String,
    /// Similarity score
    pub similarity: f64,
}

/// Executes the related tool.
///
/// # Security
///
/// - Validates path stays within root directory
/// - Blocks access to sensitive files (.env, credentials, keys)
///
/// # Errors
///
/// Returns an error string if:
/// - Path traversal is detected
/// - File is sensitive
/// - Source file cannot be read
/// - Search fails
pub fn execute_related(
    service: &Arc<SearchService>,
    input: RelatedInput,
) -> Result<RelatedOutput, String> {
    // Security: validate path and check for sensitive files
    let full_path = security::validate_read_access(service.root(), &input.path)
        .map_err(|e| e.to_string())?;

    // Read source file
    let content =
        fs::read_to_string(&full_path).map_err(|e| format!("Failed to read file: {e}"))?;

    // Extract keywords/identifiers from source
    let keywords = extract_keywords(&content);

    // Search for each keyword and aggregate results
    let mut file_scores: HashMap<String, (f64, Vec<String>)> = HashMap::new();

    for keyword in keywords.iter().take(10) {
        if let Ok(results) = service.search(keyword, 20) {
            for result in results {
                let path_str = result.path.to_string_lossy().to_string();
                if path_str == input.path {
                    continue; // Skip source file
                }

                let entry = file_scores.entry(path_str).or_insert_with(|| (0.0, vec![]));
                entry.0 += result.score.as_f64();
                if !entry.1.contains(keyword) {
                    entry.1.push(keyword.clone());
                }
            }
        }
    }

    // Sort by score and convert to output
    let mut related: Vec<_> = file_scores
        .into_iter()
        .map(|(path, (score, keywords))| {
            let relative = std::path::Path::new(&path)
                .strip_prefix(service.root())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or(path);

            let relationship = if keywords.len() > 3 {
                "strong".to_string()
            } else if keywords.len() > 1 {
                "moderate".to_string()
            } else {
                "weak".to_string()
            };

            RelatedFile {
                path: relative,
                relationship,
                similarity: (score / keywords.len() as f64).min(1.0),
            }
        })
        .collect();

    related.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    related.truncate(input.limit);

    Ok(RelatedOutput {
        source: input.path,
        related,
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

fn default_refs_limit() -> usize {
    50
}

/// Output for the refs tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RefsOutput {
    /// Symbol searched
    pub symbol: String,
    /// References found
    pub references: Vec<Reference>,
    /// Total count
    pub total: usize,
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
/// Returns an error string if the grep search fails.
pub fn execute_refs(service: &Arc<SearchService>, input: RefsInput) -> Result<RefsOutput, String> {
    // Use grep to find exact symbol matches
    let results = service
        .search_grep(
            &format!(r"\b{}\b", regex::escape(&input.symbol)),
            input.limit * 2,
        )
        .map_err(|e| e.to_string())?;

    let mut references = Vec::new();

    for result in results.into_iter().take(input.limit) {
        let full_path = &result.path;

        // Security: skip sensitive files from search results
        if security::is_sensitive_file(full_path).is_some() {
            continue;
        }

        let content = match fs::read_to_string(full_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Find lines containing the symbol
        for (line_num, line) in content.lines().enumerate() {
            if line.contains(&input.symbol) {
                let relative = full_path
                    .strip_prefix(service.root())
                    .unwrap_or(full_path)
                    .to_string_lossy()
                    .to_string();

                let ref_type = classify_reference(line, &input.symbol);

                references.push(Reference {
                    path: relative,
                    line: line_num + 1,
                    content: line.trim().to_string(),
                    ref_type,
                });

                if references.len() >= input.limit {
                    break;
                }
            }
        }

        if references.len() >= input.limit {
            break;
        }
    }

    let total = references.len();

    Ok(RefsOutput {
        symbol: input.symbol,
        references,
        total,
    })
}

// Helper functions

fn extract_keywords(content: &str) -> Vec<String> {
    let mut keywords = Vec::new();

    // Simple identifier extraction
    for word in content.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let word = word.trim();
        if word.len() >= 4
            && word.len() <= 30
            && !is_common_keyword(word)
            && word.chars().next().is_some_and(|c| c.is_alphabetic())
            && !keywords.contains(&word.to_string())
        {
            keywords.push(word.to_string());
        }
    }

    keywords
}

fn is_common_keyword(word: &str) -> bool {
    matches!(
        word.to_lowercase().as_str(),
        "function"
            | "return"
            | "const"
            | "true"
            | "false"
            | "null"
            | "undefined"
            | "string"
            | "number"
            | "boolean"
            | "import"
            | "export"
            | "default"
            | "from"
            | "this"
            | "self"
            | "async"
            | "await"
            | "public"
            | "private"
            | "protected"
            | "static"
            | "void"
            | "class"
            | "struct"
            | "enum"
            | "interface"
            | "type"
            | "impl"
            | "trait"
            | "where"
            | "match"
            | "case"
            | "break"
            | "continue"
            | "while"
            | "loop"
            | "else"
            | "elif"
            | "then"
            | "error"
            | "result"
            | "option"
            | "some"
            | "none"
    )
}

fn classify_reference(line: &str, symbol: &str) -> String {
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

    let contains_symbol = trimmed.contains(&format!(" {symbol}"))
        || trimmed.contains(&format!(" {symbol}("))
        || trimmed.contains(&format!(" {symbol}<"));

    if is_definition_keyword && contains_symbol {
        return "definition".to_string();
    }

    // Check for imports
    if trimmed.starts_with("use ")
        || trimmed.starts_with("import ")
        || trimmed.starts_with("from ")
        || trimmed.contains("require(")
    {
        return "import".to_string();
    }

    // Check for type annotations
    if trimmed.contains(&format!(": {symbol}"))
        || trimmed.contains(&format!("-> {symbol}"))
        || trimmed.contains(&format!("<{symbol}"))
    {
        return "type_usage".to_string();
    }

    "usage".to_string()
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
