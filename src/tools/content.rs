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
use std::fs;
use std::path::Path;
use std::sync::Arc;

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

fn default_start_line() -> usize {
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
    let full_path = security::validate_read_access(service.root(), &input.path)
        .map_err(|e| e.to_string())?;

    let content =
        fs::read_to_string(&full_path).map_err(|e| format!("Failed to read file: {e}"))?;

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    let start = input.start_line.saturating_sub(1).min(total_lines);
    let end = if input.end_line == 0 {
        total_lines
    } else {
        input.end_line.min(total_lines)
    };

    let selected_content = lines[start..end].join("\n");

    Ok(GetOutput {
        path: input.path,
        content: selected_content,
        total_lines,
        start_line: start + 1,
        end_line: end,
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
    /// Line number
    pub line: usize,
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
    let full_path = security::validate_read_access(service.root(), &input.path)
        .map_err(|e| e.to_string())?;

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

fn default_toc_depth() -> usize {
    3
}

/// Output for the toc tool.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TocOutput {
    /// Directory path
    pub path: String,
    /// Directory tree
    pub tree: Vec<TocEntry>,
    /// Total files
    pub total_files: usize,
    /// Total directories
    pub total_dirs: usize,
}

/// A TOC entry.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TocEntry {
    /// Entry name
    pub name: String,
    /// Entry type ("file" or "dir")
    pub entry_type: String,
    /// Relative path
    pub path: String,
    /// Children (for directories)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TocEntry>,
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
    let full_path = security::validate_path(service.root(), &input.path)
        .map_err(|e| e.to_string())?;

    let (tree, files, dirs) = build_toc(&full_path, service.root(), input.depth, 0)?;

    Ok(TocOutput {
        path: input.path,
        tree,
        total_files: files,
        total_dirs: dirs,
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

fn default_context_lines() -> usize {
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
    let full_path = security::validate_read_access(service.root(), &input.path)
        .map_err(|e| e.to_string())?;

    let content =
        fs::read_to_string(&full_path).map_err(|e| format!("Failed to read file: {e}"))?;

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    let center = input.line.saturating_sub(1).min(total.saturating_sub(1));
    let start = center.saturating_sub(input.context_lines);
    let end = (center + input.context_lines + 1).min(total);

    // Format with line numbers
    let formatted: Vec<String> = lines[start..end]
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let line_num = start + i + 1;
            let marker = if line_num == input.line { ">" } else { " " };
            format!("{marker}{line_num:4} | {line}")
        })
        .collect();

    Ok(ContextOutput {
        path: input.path,
        content: formatted.join("\n"),
        start_line: start + 1,
        end_line: end,
        center_line: input.line,
    })
}

// Helper functions

fn detect_file_type(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_else(|| "unknown".to_string())
}

fn extract_symbols(content: &str, file_type: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();

    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();
        let level = indent / 4; // Assume 4-space indentation

        let symbol = match file_type {
            "rs" => extract_rust_symbol(trimmed, line_num + 1, level),
            "py" => extract_python_symbol(trimmed, line_num + 1, level),
            "js" | "ts" | "jsx" | "tsx" => extract_js_symbol(trimmed, line_num + 1, level),
            "go" => extract_go_symbol(trimmed, line_num + 1, level),
            _ => None,
        };

        if let Some(s) = symbol {
            symbols.push(s);
        }
    }

    symbols
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
            level,
        });
    }

    None
}

fn build_toc(
    path: &Path,
    root: &Path,
    max_depth: usize,
    current_depth: usize,
) -> Result<(Vec<TocEntry>, usize, usize), String> {
    if current_depth >= max_depth || !path.is_dir() {
        return Ok((vec![], 0, 0));
    }

    let mut entries = Vec::new();
    let mut total_files = 0;
    let mut total_dirs = 0;

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

    for item in items {
        let item_path = item.path();
        let name = item.file_name().to_string_lossy().to_string();

        // Skip hidden files
        if name.starts_with('.') {
            continue;
        }

        let relative = item_path
            .strip_prefix(root)
            .unwrap_or(&item_path)
            .to_string_lossy()
            .to_string();

        if item_path.is_dir() {
            total_dirs += 1;
            let (children, sub_files, sub_dirs) =
                build_toc(&item_path, root, max_depth, current_depth + 1)?;
            total_files += sub_files;
            total_dirs += sub_dirs;

            entries.push(TocEntry {
                name,
                entry_type: "dir".to_string(),
                path: relative,
                children,
            });
        } else {
            total_files += 1;
            entries.push(TocEntry {
                name,
                entry_type: "file".to_string(),
                path: relative,
                children: vec![],
            });
        }
    }

    Ok((entries, total_files, total_dirs))
}
