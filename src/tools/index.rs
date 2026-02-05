//! Index management MCP tools.
//!
//! # Security
//!
//! The `diff` tool validates paths to prevent traversal attacks
//! and blocks access to sensitive files.
//!
//! See [`crate::security`] for details.

use crate::security;
use crate::services::{Indexer, SearchService};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Input for the index tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct IndexInput {
    /// Force full re-index
    #[serde(default)]
    pub force: bool,
}

/// Output for the index tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct IndexOutput {
    /// Files processed
    pub files_processed: usize,
    /// Files newly indexed
    pub files_indexed: usize,
    /// Files unchanged
    pub files_unchanged: usize,
    /// Files deleted from index
    pub files_deleted: usize,
    /// Status message
    pub message: String,
}

/// Executes the index tool.
///
/// # Errors
///
/// Returns an error string if indexing fails.
pub fn execute_index(indexer: &Indexer, input: IndexInput) -> Result<IndexOutput, String> {
    // Wire up progress callback for stderr logging
    let progress_cb: Option<crate::services::indexer::ProgressCallback> =
        Some(Box::new(|p: crate::services::indexer::IndexProgress| {
            eprintln!(
                "[INDEX] {}/{} files processed, {} indexed",
                p.files_processed, p.files_total, p.files_indexed
            );
        }));

    let progress = indexer
        .index(progress_cb, input.force)
        .map_err(|e| e.to_string())?;

    let message = if progress.files_indexed > 0 || progress.files_deleted > 0 {
        format!(
            "Index updated: {} new/modified, {} deleted",
            progress.files_indexed, progress.files_deleted
        )
    } else {
        "Index is up to date".to_string()
    };

    Ok(IndexOutput {
        files_processed: progress.files_processed,
        files_indexed: progress.files_indexed,
        files_unchanged: progress.files_unchanged,
        files_deleted: progress.files_deleted,
        message,
    })
}

/// Input for the diff tool.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DiffInput {
    /// First file path
    pub file1: String,
    /// Second file path
    pub file2: String,
    /// Context lines around changes
    #[serde(default = "default_diff_context")]
    pub context: usize,
}

fn default_diff_context() -> usize {
    3
}

/// Output for the diff tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DiffOutput {
    /// First file path
    pub file1: String,
    /// Second file path
    pub file2: String,
    /// Diff hunks
    pub hunks: Vec<DiffHunk>,
    /// Summary statistics
    pub stats: DiffStats,
}

/// A diff hunk.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DiffHunk {
    /// Starting line in file1
    pub old_start: usize,
    /// Number of lines in file1
    pub old_lines: usize,
    /// Starting line in file2
    pub new_start: usize,
    /// Number of lines in file2
    pub new_lines: usize,
    /// Hunk content with +/- prefixes
    pub content: String,
}

/// Diff statistics.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DiffStats {
    /// Lines added
    pub additions: usize,
    /// Lines removed
    pub deletions: usize,
    /// Lines changed
    pub changes: usize,
}

/// Executes the diff tool.
///
/// # Security
///
/// - Validates both paths stay within root directory
/// - Blocks access to sensitive files (.env, credentials, keys)
///
/// # Errors
///
/// Returns an error string if:
/// - Path traversal is detected
/// - Either file is sensitive
/// - Either file cannot be read
pub fn execute_diff(service: &Arc<SearchService>, input: DiffInput) -> Result<DiffOutput, String> {
    use std::fs;

    // Security: validate both paths and check for sensitive files
    let path1 = security::validate_read_access(service.root(), &input.file1)
        .map_err(|e| format!("file1: {e}"))?;
    let path2 = security::validate_read_access(service.root(), &input.file2)
        .map_err(|e| format!("file2: {e}"))?;

    let file1 = &input.file1;
    let content1 =
        fs::read_to_string(&path1).map_err(|e| format!("Failed to read {file1}: {e}"))?;
    let file2 = &input.file2;
    let content2 =
        fs::read_to_string(&path2).map_err(|e| format!("Failed to read {file2}: {e}"))?;

    let lines1: Vec<&str> = content1.lines().collect();
    let lines2: Vec<&str> = content2.lines().collect();

    // Simple line-by-line diff
    let (hunks, stats) = compute_diff(&lines1, &lines2, input.context);

    Ok(DiffOutput {
        file1: input.file1,
        file2: input.file2,
        hunks,
        stats,
    })
}

/// Computes a simple diff between two sets of lines.
fn compute_diff(
    old_lines: &[&str],
    new_lines: &[&str],
    context: usize,
) -> (Vec<DiffHunk>, DiffStats) {
    let mut hunks = Vec::new();
    let mut additions = 0;
    let mut deletions = 0;

    // Use a simple LCS-based diff algorithm
    let lcs = longest_common_subsequence(old_lines, new_lines);

    let mut old_idx = 0;
    let mut new_idx = 0;
    let mut hunk_lines = Vec::new();
    let mut hunk_old_start = 0;
    let mut hunk_new_start = 0;
    let mut in_hunk = false;

    for (old_match, new_match) in lcs {
        // Process deletions (lines in old but not in new before this match)
        while old_idx < old_match {
            if !in_hunk {
                hunk_old_start = old_idx + 1;
                hunk_new_start = new_idx + 1;
                in_hunk = true;

                // Add context before
                let ctx_start = old_idx.saturating_sub(context);
                for line in old_lines.iter().take(old_idx).skip(ctx_start) {
                    hunk_lines.push(format!(" {line}"));
                }
            }
            let line = old_lines[old_idx];
            hunk_lines.push(format!("-{line}"));
            deletions += 1;
            old_idx += 1;
        }

        // Process additions (lines in new but not in old before this match)
        while new_idx < new_match {
            if !in_hunk {
                hunk_old_start = old_idx + 1;
                hunk_new_start = new_idx + 1;
                in_hunk = true;
            }
            let line = new_lines[new_idx];
            hunk_lines.push(format!("+{line}"));
            additions += 1;
            new_idx += 1;
        }

        // Process matching line
        if in_hunk {
            let line = old_lines[old_idx];
            hunk_lines.push(format!(" {line}"));

            // Check if we should end the hunk
            let remaining_old = old_lines.len() - old_idx - 1;
            let remaining_new = new_lines.len() - new_idx - 1;

            if remaining_old > context && remaining_new > context {
                // End hunk and start fresh
                hunks.push(DiffHunk {
                    old_start: hunk_old_start,
                    old_lines: old_idx - hunk_old_start + 2,
                    new_start: hunk_new_start,
                    new_lines: new_idx - hunk_new_start + 2,
                    content: hunk_lines.join("\n"),
                });
                hunk_lines.clear();
                in_hunk = false;
            }
        }

        old_idx += 1;
        new_idx += 1;
    }

    // Process remaining lines
    while old_idx < old_lines.len() {
        if !in_hunk {
            hunk_old_start = old_idx + 1;
            hunk_new_start = new_idx + 1;
            in_hunk = true;
        }
        let line = old_lines[old_idx];
        hunk_lines.push(format!("-{line}"));
        deletions += 1;
        old_idx += 1;
    }

    while new_idx < new_lines.len() {
        if !in_hunk {
            hunk_old_start = old_idx + 1;
            hunk_new_start = new_idx + 1;
            in_hunk = true;
        }
        let line = new_lines[new_idx];
        hunk_lines.push(format!("+{line}"));
        additions += 1;
        new_idx += 1;
    }

    // Finalize last hunk
    if in_hunk && !hunk_lines.is_empty() {
        hunks.push(DiffHunk {
            old_start: hunk_old_start,
            old_lines: old_lines.len() - hunk_old_start + 1,
            new_start: hunk_new_start,
            new_lines: new_lines.len() - hunk_new_start + 1,
            content: hunk_lines.join("\n"),
        });
    }

    let stats = DiffStats {
        additions,
        deletions,
        changes: additions.min(deletions),
    };

    (hunks, stats)
}

/// Computes the longest common subsequence of two sequences.
/// Returns indices of matching elements: (old_index, new_index).
fn longest_common_subsequence<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let m = a.len();
    let n = b.len();

    if m == 0 || n == 0 {
        return vec![];
    }

    // Build LCS table
    let mut dp = vec![vec![0usize; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            if a[i - 1] == b[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack to find the actual LCS
    let mut result = Vec::new();
    let mut i = m;
    let mut j = n;

    while i > 0 && j > 0 {
        if a[i - 1] == b[j - 1] {
            result.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] > dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }

    result.reverse();
    result
}
