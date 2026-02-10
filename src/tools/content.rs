//! Content retrieval MCP tools.
//!
//! # Security
//!
//! All file access in this module is protected by:
//! - Path traversal validation (prevents escaping root directory)
//! - Sensitive file blocking (.env, credentials, keys, etc.)
//!
//! See [`crate::security`] for details.

use crate::security;
use crate::services::SearchService;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::Arc;

/// Wraps file content in boundary markers to help LLM consumers distinguish
/// tool metadata from untrusted file content (prompt injection defense).
fn mark_content_boundary(content: &str, path: &str) -> String {
    format!("--- BEGIN FILE CONTENT: {path} ---\n{content}\n--- END FILE CONTENT: {path} ---")
}

/// Files larger than this threshold use streaming reads.
/// Below this threshold, loading the entire file is actually faster
/// due to fewer syscalls and simpler memory allocation patterns.
const STREAMING_THRESHOLD: u64 = 100 * 1024; // 100KB

/// Input for the get tool (retrieves file content).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetInput {
    /// File path (relative to root)
    pub path: String,
    /// Starting line (1-indexed, default: 1)
    #[serde(default = "default_start_line")]
    pub start_line: usize,
    /// Ending line (0 = end of file)
    #[serde(default)]
    pub end_line: usize,
}

const fn default_start_line() -> usize {
    1
}

/// Output for the get tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GetOutput {
    /// File path
    pub path: String,
    /// File content (optionally line-bounded)
    pub content: String,
    /// Total lines in file
    pub total_lines: usize,
    /// Starting line returned
    pub start_line: usize,
    /// Ending line returned
    pub end_line: usize,
}

/// Executes the get tool.
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
/// - File cannot be read
pub fn execute_get(service: &Arc<SearchService>, input: GetInput) -> Result<GetOutput, String> {
    // Security: validate path and check for sensitive files
    let full_path =
        security::validate_read_access(service.root(), &input.path).map_err(|e| e.to_string())?;

    // Use streaming for large files to avoid loading entire file into memory
    let (content, total_lines, start_line, end_line) =
        read_line_range(&full_path, input.start_line, input.end_line)?;

    Ok(GetOutput {
        path: input.path.clone(),
        content: mark_content_boundary(&content, &input.path),
        total_lines,
        start_line,
        end_line,
    })
}

/// Input for the outline tool (extracts file structure).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct OutlineInput {
    /// File path (relative to root)
    pub path: String,
}

/// Output for the outline tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct OutlineOutput {
    /// File path
    pub path: String,
    /// Extracted symbols/structure
    pub symbols: Vec<Symbol>,
    /// File type detected
    pub file_type: String,
}

/// A symbol extracted from the file.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Symbol {
    /// Symbol name
    pub name: String,
    /// Symbol kind (function, class, struct, etc.)
    pub kind: String,
    /// Start line number
    pub line: usize,
    /// End line number (if detectable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    /// Indentation level
    pub level: usize,
}

/// Executes the outline tool.
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
/// - File cannot be read
pub fn execute_outline(
    service: &Arc<SearchService>,
    input: OutlineInput,
) -> Result<OutlineOutput, String> {
    // Security: validate path and check for sensitive files
    let full_path =
        security::validate_read_access(service.root(), &input.path).map_err(|e| e.to_string())?;

    let content =
        fs::read_to_string(&full_path).map_err(|e| format!("Failed to read file: {e}"))?;

    let file_type = detect_file_type(&full_path);
    let symbols = extract_symbols(&content, &file_type);

    Ok(OutlineOutput {
        path: input.path,
        symbols,
        file_type,
    })
}

/// Input for the toc tool (table of contents for directory).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TocInput {
    /// Directory path (relative to root, default: ".")
    #[serde(default = "default_toc_path")]
    pub path: String,
    /// Maximum depth (default: 3)
    #[serde(default = "default_toc_depth")]
    pub depth: usize,
}

fn default_toc_path() -> String {
    ".".to_string()
}

const fn default_toc_depth() -> usize {
    3
}

/// Output for the toc tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TocOutput {
    /// Indented directory tree (like `tree` command output)
    pub tree: String,
    /// Total files
    pub total_files: usize,
    /// Total directories
    pub total_dirs: usize,
}

/// Executes the toc tool.
///
/// # Security
///
/// - Validates path stays within root directory
///
/// # Errors
///
/// Returns an error string if:
/// - Path traversal is detected
/// - Directory cannot be read
pub fn execute_toc(service: &Arc<SearchService>, input: TocInput) -> Result<TocOutput, String> {
    // Security: validate path (toc reads directories, so we use validate_path not validate_read_access)
    let full_path =
        security::validate_path(service.root(), &input.path).map_err(|e| e.to_string())?;

    let mut tree = String::with_capacity(4096); // Pre-allocate, ~40 bytes per entry
    let mut total_files = 0;
    let mut total_dirs = 0;

    build_toc_text(
        &full_path,
        input.depth,
        0,
        &mut tree,
        &mut total_files,
        &mut total_dirs,
    )?;

    Ok(TocOutput {
        tree,
        total_files,
        total_dirs,
    })
}

/// Input for the context tool (gets surrounding context).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ContextInput {
    /// File path
    pub path: String,
    /// Center line number
    pub line: usize,
    /// Lines of context before and after (default: 10)
    #[serde(default = "default_context_lines")]
    pub context_lines: usize,
}

const fn default_context_lines() -> usize {
    10
}

/// Output for the context tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ContextOutput {
    /// File path
    pub path: String,
    /// Context content with line numbers
    pub content: String,
    /// Start line
    pub start_line: usize,
    /// End line
    pub end_line: usize,
    /// Center line
    pub center_line: usize,
}

/// Executes the context tool.
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
/// - File cannot be read
pub fn execute_context(
    service: &Arc<SearchService>,
    input: ContextInput,
) -> Result<ContextOutput, String> {
    // Security: validate path and check for sensitive files
    let full_path =
        security::validate_read_access(service.root(), &input.path).map_err(|e| e.to_string())?;

    // Use streaming for large files to avoid loading entire file into memory
    let (lines, _total, start, end) =
        read_context_streaming(&full_path, input.line, input.context_lines)?;

    // Format with line numbers
    let formatted: Vec<String> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let line_num = start + i + 1;
            let marker = if line_num == input.line { ">" } else { " " };
            format!("{marker}{line_num:4} | {line}")
        })
        .collect();

    let content = formatted.join("\n");
    Ok(ContextOutput {
        path: input.path.clone(),
        content: mark_content_boundary(&content, &input.path),
        start_line: start + 1,
        end_line: end,
        center_line: input.line,
    })
}

// Helper functions

/// Reads a line range from a file using streaming for large files.
///
/// For files over `STREAMING_THRESHOLD`, this uses `BufReader` to avoid
/// loading the entire file into memory. For smaller files, it falls back
/// to the simpler read-all approach which is actually faster due to
/// fewer syscalls.
///
/// Returns `(content, total_lines, actual_start, actual_end)`.
fn read_line_range(
    path: &Path,
    start_line: usize,
    end_line: usize,
) -> Result<(String, usize, usize, usize), String> {
    let metadata = fs::metadata(path).map_err(|e| format!("Failed to read file metadata: {e}"))?;

    if metadata.len() > STREAMING_THRESHOLD {
        read_line_range_streaming(path, start_line, end_line)
    } else {
        read_line_range_full(path, start_line, end_line)
    }
}

/// Streaming implementation for large files.
fn read_line_range_streaming(
    path: &Path,
    start_line: usize,
    end_line: usize,
) -> Result<(String, usize, usize, usize), String> {
    let file = File::open(path).map_err(|e| format!("Failed to open file: {e}"))?;
    let reader = BufReader::new(file);

    let mut selected = Vec::new();
    let mut total_lines = 0;

    // Convert to 0-indexed
    let start = start_line.saturating_sub(1);
    let end = if end_line == 0 { usize::MAX } else { end_line };

    for (i, line_result) in reader.lines().enumerate() {
        total_lines = i + 1;
        let line_num = i + 1; // 1-indexed

        if line_num > start && line_num <= end {
            let line = line_result.map_err(|e| format!("Failed to read line: {e}"))?;
            selected.push(line);
        }
        // Continue iterating to get accurate total_lines count
    }

    let actual_start = start.min(total_lines.saturating_sub(1)) + 1;
    let actual_end = end.min(total_lines);

    Ok((selected.join("\n"), total_lines, actual_start, actual_end))
}

/// Full-file read for small files (faster due to fewer syscalls).
fn read_line_range_full(
    path: &Path,
    start_line: usize,
    end_line: usize,
) -> Result<(String, usize, usize, usize), String> {
    let content = fs::read_to_string(path).map_err(|e| format!("Failed to read file: {e}"))?;

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    let start = start_line.saturating_sub(1).min(total_lines);
    let end = if end_line == 0 {
        total_lines
    } else {
        end_line.min(total_lines)
    };

    let selected_content = lines[start..end].join("\n");

    Ok((selected_content, total_lines, start + 1, end))
}

/// Reads context around a line using streaming for large files.
fn read_context_streaming(
    path: &Path,
    center_line: usize,
    context_lines: usize,
) -> Result<(Vec<String>, usize, usize, usize), String> {
    let metadata = fs::metadata(path).map_err(|e| format!("Failed to read file metadata: {e}"))?;

    if metadata.len() > STREAMING_THRESHOLD {
        read_context_streaming_impl(path, center_line, context_lines)
    } else {
        read_context_full(path, center_line, context_lines)
    }
}

/// Streaming implementation for context reading.
///
/// Stops reading as soon as we've collected all lines up to `end_target`,
/// rather than iterating to EOF just to count total lines (which the
/// caller doesn't use).
fn read_context_streaming_impl(
    path: &Path,
    center_line: usize,
    context_lines: usize,
) -> Result<(Vec<String>, usize, usize, usize), String> {
    let file = File::open(path).map_err(|e| format!("Failed to open file: {e}"))?;
    let reader = BufReader::new(file);

    let center = center_line.saturating_sub(1);
    let start = center.saturating_sub(context_lines);
    let end_target = center + context_lines + 1;

    let mut lines_buffer = Vec::new();
    let mut last_line_seen = 0;

    for (i, line_result) in reader.lines().enumerate() {
        last_line_seen = i + 1;

        if i >= start && i < end_target {
            let line = line_result.map_err(|e| format!("Failed to read line: {e}"))?;
            lines_buffer.push(line);
        }

        // Stop once we've passed the context window — no need to read to EOF
        if i >= end_target {
            break;
        }
    }

    let actual_end = end_target.min(last_line_seen);

    Ok((lines_buffer, last_line_seen, start, actual_end))
}

/// Full-file read for context (small files).
fn read_context_full(
    path: &Path,
    center_line: usize,
    context_lines: usize,
) -> Result<(Vec<String>, usize, usize, usize), String> {
    let content = fs::read_to_string(path).map_err(|e| format!("Failed to read file: {e}"))?;

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    let center = center_line.saturating_sub(1).min(total.saturating_sub(1));
    let start = center.saturating_sub(context_lines);
    let end = (center + context_lines + 1).min(total);

    let selected: Vec<String> = lines[start..end].iter().map(|s| (*s).to_string()).collect();

    Ok((selected, total, start, end))
}

fn detect_file_type(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_else(|| "unknown".to_string())
}

fn extract_symbols(content: &str, file_type: &str) -> Vec<Symbol> {
    let lines: Vec<&str> = content.lines().collect();
    let mut symbols = Vec::new();

    for (line_num, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();
        let level = indent / 4; // Assume 4-space indentation

        let symbol = match file_type {
            "rs" | "go" | "js" | "ts" | "jsx" | "tsx" => {
                let raw = match file_type {
                    "rs" => extract_rust_symbol(trimmed, line_num + 1, level),
                    "go" => extract_go_symbol(trimmed, line_num + 1, level),
                    _ => extract_js_symbol(trimmed, line_num + 1, level),
                };
                // Compute end_line via brace tracking for brace-delimited languages
                raw.map(|mut s| {
                    s.end_line = find_brace_end(&lines, line_num);
                    s
                })
            }
            "py" => {
                let raw = extract_python_symbol(trimmed, line_num + 1, level);
                // Compute end_line via indent tracking for Python
                raw.map(|mut s| {
                    s.end_line = find_indent_end(&lines, line_num, indent);
                    s
                })
            }
            _ => None,
        };

        if let Some(s) = symbol {
            symbols.push(s);
        }
    }

    symbols
}

/// Finds the closing brace for a symbol starting at `start_line` (0-indexed).
/// Returns the 1-indexed line number of the closing brace, or None.
fn find_brace_end(lines: &[&str], start_line: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut found_open = false;

    for (i, line) in lines.iter().enumerate().skip(start_line) {
        for ch in line.chars() {
            if ch == '{' {
                depth += 1;
                found_open = true;
            } else if ch == '}' {
                depth -= 1;
                if found_open && depth == 0 {
                    return Some(i + 1); // 1-indexed
                }
            }
        }
        // If we found the opening brace but are past it and depth is back to 0
        if found_open && depth == 0 {
            return Some(i + 1);
        }
    }

    None
}

/// Finds the end of an indentation block for Python (0-indexed start line).
/// Returns the 1-indexed line number of the last line in the block.
fn find_indent_end(lines: &[&str], start_line: usize, base_indent: usize) -> Option<usize> {
    let mut last_content_line = start_line;

    for (i, line) in lines.iter().enumerate().skip(start_line + 1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue; // Skip blank lines
        }
        let indent = line.len() - line.trim_start().len();
        if indent <= base_indent {
            // We've exited the block
            return Some(last_content_line + 1); // 1-indexed
        }
        last_content_line = i;
    }

    // Block extends to end of file
    Some(last_content_line + 1)
}

fn extract_rust_symbol(line: &str, line_num: usize, level: usize) -> Option<Symbol> {
    if line.starts_with("fn ") || line.starts_with("pub fn ") || line.starts_with("async fn ") {
        let name = line
            .split('(')
            .next()?
            .split_whitespace()
            .last()?
            .to_string();
        return Some(Symbol {
            name,
            kind: "function".to_string(),
            line: line_num,
            end_line: None,
            level,
        });
    }

    if line.starts_with("struct ") || line.starts_with("pub struct ") {
        let name = line
            .split('{')
            .next()?
            .split('<')
            .next()?
            .split_whitespace()
            .last()?
            .to_string();
        return Some(Symbol {
            name,
            kind: "struct".to_string(),
            line: line_num,
            end_line: None,
            level,
        });
    }

    if line.starts_with("enum ") || line.starts_with("pub enum ") {
        let name = line
            .split('{')
            .next()?
            .split_whitespace()
            .last()?
            .to_string();
        return Some(Symbol {
            name,
            kind: "enum".to_string(),
            line: line_num,
            end_line: None,
            level,
        });
    }

    if line.starts_with("impl ") || line.starts_with("impl<") {
        let name = line
            .split('{')
            .next()?
            .trim()
            .trim_start_matches("impl")
            .trim_start_matches('<')
            .split('>')
            .next_back()?
            .trim()
            .to_string();
        return Some(Symbol {
            name,
            kind: "impl".to_string(),
            line: line_num,
            end_line: None,
            level,
        });
    }

    if line.starts_with("trait ") || line.starts_with("pub trait ") {
        let name = line
            .split('{')
            .next()?
            .split('<')
            .next()?
            .split_whitespace()
            .last()?
            .to_string();
        return Some(Symbol {
            name,
            kind: "trait".to_string(),
            line: line_num,
            end_line: None,
            level,
        });
    }

    if line.starts_with("mod ") || line.starts_with("pub mod ") {
        let name = line
            .split('{')
            .next()?
            .split(';')
            .next()?
            .split_whitespace()
            .last()?
            .to_string();
        return Some(Symbol {
            name,
            kind: "module".to_string(),
            line: line_num,
            end_line: None,
            level,
        });
    }

    None
}

fn extract_python_symbol(line: &str, line_num: usize, level: usize) -> Option<Symbol> {
    if line.starts_with("def ") {
        let name = line
            .trim_start_matches("def ")
            .split('(')
            .next()?
            .to_string();
        return Some(Symbol {
            name,
            kind: "function".to_string(),
            line: line_num,
            end_line: None,
            level,
        });
    }

    if line.starts_with("class ") {
        let name = line
            .trim_start_matches("class ")
            .split('(')
            .next()?
            .split(':')
            .next()?
            .to_string();
        return Some(Symbol {
            name,
            kind: "class".to_string(),
            line: line_num,
            end_line: None,
            level,
        });
    }

    if line.starts_with("async def ") {
        let name = line
            .trim_start_matches("async def ")
            .split('(')
            .next()?
            .to_string();
        return Some(Symbol {
            name,
            kind: "async_function".to_string(),
            line: line_num,
            end_line: None,
            level,
        });
    }

    None
}

fn extract_js_symbol(line: &str, line_num: usize, level: usize) -> Option<Symbol> {
    if line.starts_with("function ")
        || line.starts_with("async function ")
        || line.starts_with("export function ")
        || line.starts_with("export async function ")
    {
        let name = line
            .split('(')
            .next()?
            .split_whitespace()
            .last()?
            .to_string();
        return Some(Symbol {
            name,
            kind: "function".to_string(),
            line: line_num,
            end_line: None,
            level,
        });
    }

    if line.starts_with("class ") || line.starts_with("export class ") {
        let name = line
            .split('{')
            .next()?
            .split_whitespace()
            .find(|w| *w != "class" && *w != "export" && *w != "extends")?
            .to_string();
        return Some(Symbol {
            name,
            kind: "class".to_string(),
            line: line_num,
            end_line: None,
            level,
        });
    }

    if line.contains("const ")
        && line.contains(" = ")
        && (line.contains("=>") || line.contains("function"))
    {
        let name = line
            .split('=')
            .next()?
            .split_whitespace()
            .last()?
            .to_string();
        return Some(Symbol {
            name,
            kind: "function".to_string(),
            line: line_num,
            end_line: None,
            level,
        });
    }

    None
}

fn extract_go_symbol(line: &str, line_num: usize, level: usize) -> Option<Symbol> {
    if line.starts_with("func ") {
        let rest = line.trim_start_matches("func ");
        // Handle method syntax: func (r *Receiver) MethodName()
        let name = if rest.starts_with('(') {
            rest.split(')')
                .nth(1)?
                .trim()
                .split('(')
                .next()?
                .to_string()
        } else {
            rest.split('(').next()?.to_string()
        };
        return Some(Symbol {
            name,
            kind: "function".to_string(),
            line: line_num,
            end_line: None,
            level,
        });
    }

    if line.starts_with("type ") && line.contains(" struct") {
        let name = line
            .trim_start_matches("type ")
            .split_whitespace()
            .next()?
            .to_string();
        return Some(Symbol {
            name,
            kind: "struct".to_string(),
            line: line_num,
            end_line: None,
            level,
        });
    }

    if line.starts_with("type ") && line.contains(" interface") {
        let name = line
            .trim_start_matches("type ")
            .split_whitespace()
            .next()?
            .to_string();
        return Some(Symbol {
            name,
            kind: "interface".to_string(),
            line: line_num,
            end_line: None,
            level,
        });
    }

    None
}

/// Builds an indented text tree, writing directly into the output string.
fn build_toc_text(
    path: &Path,
    max_depth: usize,
    current_depth: usize,
    output: &mut String,
    total_files: &mut usize,
    total_dirs: &mut usize,
) -> Result<(), String> {
    if current_depth >= max_depth || !path.is_dir() {
        return Ok(());
    }

    let mut items: Vec<_> = fs::read_dir(path)
        .map_err(|e| format!("Failed to read directory: {e}"))?
        .filter_map(Result::ok)
        .collect();

    // Sort: directories first, then alphabetically
    items.sort_by(|a, b| {
        let a_is_dir = a.path().is_dir();
        let b_is_dir = b.path().is_dir();
        match (a_is_dir, b_is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.file_name().cmp(&b.file_name()),
        }
    });

    let indent = "  ".repeat(current_depth);

    for item in items {
        let item_path = item.path();
        let name = item.file_name().to_string_lossy().to_string();

        // Skip hidden files
        if name.starts_with('.') {
            continue;
        }

        if item_path.is_dir() {
            *total_dirs += 1;
            output.push_str(&indent);
            output.push_str(&name);
            output.push_str("/\n");
            build_toc_text(
                &item_path,
                max_depth,
                current_depth + 1,
                output,
                total_files,
                total_dirs,
            )?;
        } else {
            *total_files += 1;
            output.push_str(&indent);
            output.push_str(&name);
            output.push('\n');
        }
    }

    Ok(())
}
