//! Structural code search backed by the ast-grep Rust API.
//!
//! This module owns candidate-file filtering and native ast-grep matching. The
//! MCP tool layer stays thin so tests can inject alternative backends while the
//! production path avoids process startup and JSON parsing overhead.

use crate::error::ServerError;
use crate::security;
use ast_grep_core::matcher::{KindMatcher, Pattern};
use ast_grep_core::{Doc, MatchStrictness, NodeMatch};
use ast_grep_language::{LanguageExt, SupportLang};
use ignore::overrides::{Override, OverrideBuilder};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// Default maximum number of structural matches returned.
pub const DEFAULT_LIMIT: usize = 20;
/// Hard maximum number of structural matches returned.
pub const MAX_LIMIT: usize = 200;
/// Default timeout for a structural search operation.
pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;
/// Hard maximum timeout for a structural search operation.
pub const MAX_TIMEOUT_MS: u64 = 30_000;

const MAX_QUERY_BYTES: usize = 4096;
const MAX_SELECTOR_BYTES: usize = 256;
const MAX_GLOBS: usize = 32;
const MAX_GLOB_BYTES: usize = 256;
const MAX_CANDIDATES: usize = 20_000;
const MAX_SNIPPET_BYTES: usize = 800;
const MAX_META_BYTES: usize = 4096;
const MAX_FILE_SIZE: u64 = 1024 * 1024;

/// Language to parse with ast-grep.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StructuralLanguage {
    /// Rust source files (`.rs`).
    Rust,
    /// Python source files (`.py`, `.pyw`).
    Python,
    /// Go source files (`.go`).
    Go,
    /// JavaScript source files (`.js`, `.mjs`, `.cjs`).
    Javascript,
    /// JSX source files (`.jsx`).
    Jsx,
    /// TypeScript source files (`.ts`, `.mts`, `.cts`).
    Typescript,
    /// TSX source files (`.tsx`).
    Tsx,
}

impl StructuralLanguage {
    fn support_lang(self) -> SupportLang {
        match self {
            Self::Rust => SupportLang::Rust,
            Self::Python => SupportLang::Python,
            Self::Go => SupportLang::Go,
            Self::Javascript | Self::Jsx => SupportLang::JavaScript,
            Self::Typescript => SupportLang::TypeScript,
            Self::Tsx => SupportLang::Tsx,
        }
    }

    fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Rust => &["rs"],
            Self::Python => &["py", "pyw"],
            Self::Go => &["go"],
            Self::Javascript => &["js", "mjs", "cjs"],
            Self::Jsx => &["jsx"],
            Self::Typescript => &["ts", "mts", "cts"],
            Self::Tsx => &["tsx"],
        }
    }

    fn matches_path(self, path: &Path) -> bool {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return false;
        };
        self.extensions()
            .iter()
            .any(|candidate| ext.eq_ignore_ascii_case(candidate))
    }
}

/// ast-grep pattern strictness.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StructuralStrictness {
    /// Match concrete syntax tree nodes.
    Cst,
    /// ast-grep default smart matching.
    Smart,
    /// Match abstract syntax tree nodes.
    Ast,
    /// Relaxed structural matching.
    Relaxed,
    /// Signature-style matching.
    Signature,
}

impl StructuralStrictness {
    const fn match_strictness(self) -> MatchStrictness {
        match self {
            Self::Cst => MatchStrictness::Cst,
            Self::Smart => MatchStrictness::Smart,
            Self::Ast => MatchStrictness::Ast,
            Self::Relaxed => MatchStrictness::Relaxed,
            Self::Signature => MatchStrictness::Signature,
        }
    }
}

/// Structural query variant.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StructuralQuery {
    /// Match code using ast-grep pattern syntax.
    Pattern {
        /// ast-grep pattern, for example `fn $NAME($$$ARGS) { $$$BODY }`.
        pattern: String,
        /// Optional AST selector used to match a sub-node of the pattern.
        selector: Option<String>,
    },
    /// Match nodes by tree-sitter kind, for example `function_item`.
    Kind {
        /// Tree-sitter node kind.
        kind: String,
    },
}

impl StructuralQuery {
    fn validate(&self) -> Result<(), ServerError> {
        match self {
            Self::Pattern { pattern, selector } => {
                validate_non_empty("pattern", pattern, MAX_QUERY_BYTES)?;
                if let Some(selector) = selector {
                    validate_non_empty("selector", selector, MAX_SELECTOR_BYTES)?;
                }
            }
            Self::Kind { kind } => {
                validate_non_empty("kind", kind, MAX_SELECTOR_BYTES)?;
            }
        }
        Ok(())
    }
}

/// Request for structural search.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct StructuralSearchInput {
    /// Language to parse.
    pub language: StructuralLanguage,
    /// Pattern or node-kind query.
    pub query: StructuralQuery,
    /// Relative file or directory scope. Defaults to workspace root.
    #[serde(default = "default_path")]
    pub path: String,
    /// Optional include/exclude globs, interpreted like ripgrep overrides.
    #[serde(default)]
    pub globs: Vec<String>,
    /// Maximum matches. Default 20, max 200.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Include ast-grep metavariable captures.
    #[serde(default)]
    pub include_meta: bool,
    /// Optional ast-grep strictness.
    pub strictness: Option<StructuralStrictness>,
    /// Structural search timeout in milliseconds. Default 10000, max 30000.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_path() -> String {
    ".".to_string()
}

const fn default_limit() -> usize {
    DEFAULT_LIMIT
}

const fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

/// Output for structural search.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StructuralSearchOutput {
    /// Structural matches.
    #[serde(rename = "r")]
    pub results: Vec<StructuralSearchHit>,
    /// True when more matches exist.
    #[serde(rename = "more")]
    #[serde(skip_serializing_if = "is_false")]
    pub has_more: bool,
    /// Number of eligible candidate files considered.
    #[serde(rename = "scanned")]
    pub scanned_files: usize,
    /// Guidance for empty or incomplete results.
    #[serde(rename = "hint")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// A single structural search match.
#[derive(Debug, Serialize, JsonSchema, PartialEq)]
pub struct StructuralSearchHit {
    /// File path relative to workspace root.
    #[serde(rename = "p")]
    pub path: String,
    /// Start line, 1-indexed.
    #[serde(rename = "l")]
    pub line: usize,
    /// End line, 1-indexed.
    #[serde(rename = "end")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    /// Start column, 1-indexed.
    #[serde(rename = "c")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
    /// Matching code line or node text.
    #[serde(rename = "t")]
    pub text: String,
    /// Language reported by ast-grep.
    #[serde(rename = "lang")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Optional ast-grep metavariable captures.
    #[serde(rename = "meta")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

const fn is_false(v: &bool) -> bool {
    !*v
}

/// Backend abstraction for structural search.
pub trait StructuralSearchBackend {
    /// Executes structural search against `root`.
    ///
    /// # Errors
    ///
    /// Returns a `ServerError` when validation, candidate collection, source
    /// parsing, or matching fails.
    fn search(
        &self,
        root: &Path,
        input: StructuralSearchInput,
    ) -> crate::error::Result<StructuralSearchOutput>;
}

/// Native ast-grep backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct AstGrepNativeBackend;

impl StructuralSearchBackend for AstGrepNativeBackend {
    fn search(
        &self,
        root: &Path,
        mut input: StructuralSearchInput,
    ) -> crate::error::Result<StructuralSearchOutput> {
        validate_input(&mut input)?;

        let candidates = collect_candidates(root, &input)?;
        if candidates.is_empty() {
            return Ok(StructuralSearchOutput {
                results: Vec::new(),
                has_more: false,
                scanned_files: 0,
                hint: Some(
                    "No eligible files found for the requested language, path, and globs."
                        .to_string(),
                ),
            });
        }

        let lang = input.language.support_lang();
        let matcher = NativeMatcher::build(lang, &input)?;
        let timeout = Duration::from_millis(input.timeout_ms);
        let start = Instant::now();
        let mut results = Vec::with_capacity(input.limit.min(DEFAULT_LIMIT));
        let mut has_more = false;

        'files: for candidate in &candidates {
            ensure_not_timed_out(start, timeout)?;
            let source = read_candidate(root, candidate)?;
            let ast = lang.ast_grep(&source);
            let root_node = ast.root();

            match &matcher {
                NativeMatcher::Pattern(pattern) => {
                    for matched in root_node.find_all(pattern) {
                        if results.len() == input.limit {
                            has_more = true;
                            break 'files;
                        }
                        results.push(hit_from_match(
                            candidate,
                            input.language,
                            &matched,
                            input.include_meta,
                        ));
                    }
                }
                NativeMatcher::Kind(kind) => {
                    for matched in root_node.find_all(kind) {
                        if results.len() == input.limit {
                            has_more = true;
                            break 'files;
                        }
                        results.push(hit_from_match(
                            candidate,
                            input.language,
                            &matched,
                            input.include_meta,
                        ));
                    }
                }
            }
        }

        let hint = if results.is_empty() {
            Some(
                "No structural matches found. Check that the pattern is valid code for the selected language."
                    .to_string(),
            )
        } else {
            None
        };

        Ok(StructuralSearchOutput {
            results,
            has_more,
            scanned_files: candidates.len(),
            hint,
        })
    }
}

fn validate_input(input: &mut StructuralSearchInput) -> Result<(), ServerError> {
    input.query.validate()?;

    if input.path.trim().is_empty() {
        input.path = default_path();
    }

    if input.limit == 0 {
        input.limit = DEFAULT_LIMIT;
    }
    input.limit = input.limit.min(MAX_LIMIT);

    if input.timeout_ms == 0 {
        input.timeout_ms = DEFAULT_TIMEOUT_MS;
    }
    input.timeout_ms = input.timeout_ms.min(MAX_TIMEOUT_MS);

    if input.globs.len() > MAX_GLOBS {
        return Err(ServerError::Tool(format!(
            "Too many globs: maximum is {MAX_GLOBS}"
        )));
    }
    for glob in &input.globs {
        validate_non_empty("glob", glob, MAX_GLOB_BYTES)?;
    }

    Ok(())
}

fn validate_non_empty(name: &str, value: &str, max_bytes: usize) -> Result<(), ServerError> {
    if value.trim().is_empty() {
        return Err(ServerError::Tool(format!("{name} must not be empty")));
    }
    if value.len() > max_bytes {
        return Err(ServerError::Tool(format!(
            "{name} is too long: maximum is {max_bytes} bytes"
        )));
    }
    if value.contains('\0') {
        return Err(ServerError::Tool(format!(
            "{name} must not contain null bytes"
        )));
    }
    Ok(())
}

fn collect_candidates(
    root: &Path,
    input: &StructuralSearchInput,
) -> crate::error::Result<Vec<String>> {
    let start = security::validate_path(root, &input.path)?;
    if !start.exists() {
        return Err(ServerError::Tool(format!(
            "Path '{}' does not exist",
            input.path
        )));
    }

    let overrides = build_overrides(root, &input.globs)?;

    if start.is_file() {
        security::validate_read_access(root, &input.path)?;
        if !should_search_file(root, &start, input.language, &overrides)? {
            return Ok(Vec::new());
        }
        return Ok(vec![relative_path(root, &start)?]);
    }

    if !start.is_dir() {
        return Err(ServerError::Tool(format!(
            "Path '{}' is not a file or directory",
            input.path
        )));
    }

    let mut files = Vec::new();
    let walker = ignore::WalkBuilder::new(&start)
        .hidden(true)
        .follow_links(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .max_filesize(Some(MAX_FILE_SIZE))
        .build();

    for entry in walker.filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        if should_search_file(root, path, input.language, &overrides)? {
            files.push(relative_path(root, path)?);
            if files.len() > MAX_CANDIDATES {
                return Err(ServerError::Tool(format!(
                    "Too many candidate files ({MAX_CANDIDATES}+). Narrow path or globs."
                )));
            }
        }
    }

    files.sort();
    Ok(files)
}

fn build_overrides(root: &Path, globs: &[String]) -> crate::error::Result<Override> {
    if globs.is_empty() {
        return Ok(Override::empty());
    }

    let mut builder = OverrideBuilder::new(root);
    for glob in globs {
        builder
            .add(glob)
            .map_err(|e| ServerError::Tool(format!("Invalid glob '{glob}': {e}")))?;
    }
    builder
        .build()
        .map_err(|e| ServerError::Tool(format!("Invalid globs: {e}")))
}

fn should_search_file(
    root: &Path,
    path: &Path,
    language: StructuralLanguage,
    overrides: &Override,
) -> crate::error::Result<bool> {
    if !language.matches_path(path) {
        return Ok(false);
    }

    if security::is_sensitive_file(path).is_some() {
        return Ok(false);
    }

    let rel = relative_path(root, path)?;
    if security::is_sensitive_file(Path::new(&rel)).is_some() {
        return Ok(false);
    }

    if !overrides.is_empty() && overrides.matched(Path::new(&rel), false).is_ignore() {
        return Ok(false);
    }

    Ok(true)
}

fn relative_path(root: &Path, path: &Path) -> crate::error::Result<String> {
    if let Ok(rel) = path.strip_prefix(root) {
        return Ok(rel.to_string_lossy().replace('\\', "/"));
    }

    let canonical_root = dunce::canonicalize(root)?;
    let canonical_path = dunce::canonicalize(path)?;
    let rel = canonical_path
        .strip_prefix(&canonical_root)
        .map_err(|_| ServerError::Tool("Candidate path escaped workspace root".to_string()))?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

enum NativeMatcher {
    Pattern(Pattern),
    Kind(KindMatcher),
}

impl NativeMatcher {
    fn build(lang: SupportLang, input: &StructuralSearchInput) -> crate::error::Result<Self> {
        match &input.query {
            StructuralQuery::Pattern { pattern, selector } => {
                let pattern = if let Some(selector) = selector {
                    Pattern::contextual(pattern, selector, lang)
                } else {
                    Pattern::try_new(pattern, lang)
                }
                .map_err(|e| ServerError::Tool(format!("Invalid ast-grep pattern: {e}")))?;

                let pattern = if let Some(strictness) = input.strictness {
                    pattern.with_strictness(strictness.match_strictness())
                } else {
                    pattern
                };
                Ok(Self::Pattern(pattern))
            }
            StructuralQuery::Kind { kind } => KindMatcher::try_new(kind, lang)
                .map(Self::Kind)
                .map_err(|e| ServerError::Tool(format!("Invalid ast-grep kind: {e}"))),
        }
    }
}

fn ensure_not_timed_out(start: Instant, timeout: Duration) -> crate::error::Result<()> {
    if start.elapsed() >= timeout {
        return Err(ServerError::Tool(format!(
            "structural search timed out after {} ms. Narrow path or globs.",
            timeout.as_millis()
        )));
    }
    Ok(())
}

fn read_candidate(root: &Path, candidate: &str) -> crate::error::Result<String> {
    let path = root.join(candidate);
    std::fs::read_to_string(&path).map_err(|e| {
        ServerError::Tool(format!(
            "Failed to read structural search candidate '{}': {e}",
            candidate
        ))
    })
}

fn hit_from_match<D: Doc>(
    path: &str,
    language: StructuralLanguage,
    matched: &NodeMatch<'_, D>,
    include_meta: bool,
) -> StructuralSearchHit {
    let node = matched.get_node();
    let start = node.start_pos();
    let end = node.end_pos();
    let line = start.line().saturating_add(1);
    let end = end.line().saturating_add(1);

    StructuralSearchHit {
        path: path.to_string(),
        line,
        end_line: (end != line).then_some(end),
        column: Some(start.column(node).saturating_add(1)),
        text: truncate_text(matched.text().trim(), MAX_SNIPPET_BYTES),
        language: Some(language.support_lang().to_string()),
        meta: include_meta.then(|| meta_from_match(matched)).flatten(),
    }
}

fn meta_from_match<D: Doc>(matched: &NodeMatch<'_, D>) -> Option<Value> {
    let captures: std::collections::HashMap<String, String> = matched.get_env().clone().into();
    if captures.is_empty() {
        return None;
    }

    let captures: BTreeMap<_, _> = captures.into_iter().collect();
    serde_json::to_value(captures).ok().and_then(cap_meta)
}

fn truncate_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let cut = text.floor_char_boundary(max_bytes);
    let mut truncated = text[..cut].to_string();
    truncated.push_str("...");
    truncated
}

fn cap_meta(value: Value) -> Option<Value> {
    match serde_json::to_string(&value) {
        Ok(json) if json.len() <= MAX_META_BYTES => Some(value),
        Ok(_) => Some(serde_json::json!({ "truncated": true })),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn rust_input(query: StructuralQuery) -> StructuralSearchInput {
        StructuralSearchInput {
            language: StructuralLanguage::Rust,
            query,
            path: ".".to_string(),
            globs: Vec::new(),
            limit: 20,
            include_meta: false,
            strictness: None,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }

    #[test]
    fn native_pattern_search_returns_locations_and_captures() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path().join("src/lib.rs").as_path(),
            "fn main() {}\nfn helper() {}\n",
        );

        let mut input = rust_input(StructuralQuery::Pattern {
            pattern: "fn $NAME() {}".to_string(),
            selector: None,
        });
        input.include_meta = true;
        input.limit = 1;

        let output = AstGrepNativeBackend.search(dir.path(), input).unwrap();

        assert_eq!(output.results.len(), 1);
        assert!(output.has_more);
        let hit = &output.results[0];
        assert_eq!(hit.path, "src/lib.rs");
        assert_eq!(hit.line, 1);
        assert_eq!(hit.end_line, None);
        assert_eq!(hit.column, Some(1));
        assert_eq!(hit.text, "fn main() {}");
        assert_eq!(hit.language.as_deref(), Some("Rust"));
        assert_eq!(
            hit.meta.as_ref().and_then(|meta| meta.get("NAME")),
            Some(&Value::String("main".to_string()))
        );
    }

    #[test]
    fn native_kind_search_uses_ast_node_kinds() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path().join("src/lib.rs").as_path(),
            "struct App;\nfn build() -> App { App }\n",
        );

        let input = rust_input(StructuralQuery::Kind {
            kind: "function_item".to_string(),
        });

        let output = AstGrepNativeBackend.search(dir.path(), input).unwrap();

        assert_eq!(output.results.len(), 1);
        assert_eq!(output.results[0].line, 2);
        assert_eq!(output.results[0].text, "fn build() -> App { App }");
    }

    #[test]
    fn collect_candidates_respects_language_gitignore_and_sensitive_files() {
        let dir = TempDir::new().unwrap();
        write(dir.path().join("main.rs").as_path(), "fn main() {}\n");
        write(dir.path().join("ignored.rs").as_path(), "fn ignored() {}\n");
        write(dir.path().join(".env.rs").as_path(), "SECRET=1\n");
        write(dir.path().join("main.py").as_path(), "def main(): pass\n");
        fs::create_dir(dir.path().join(".git")).unwrap();
        write(dir.path().join(".gitignore").as_path(), "ignored.rs\n");

        let input = StructuralSearchInput {
            language: StructuralLanguage::Rust,
            query: StructuralQuery::Kind {
                kind: "function_item".to_string(),
            },
            path: ".".to_string(),
            globs: Vec::new(),
            limit: 20,
            include_meta: false,
            strictness: None,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        };

        let candidates = collect_candidates(dir.path(), &input).unwrap();

        assert_eq!(candidates, vec!["main.rs"]);
    }

    #[test]
    fn collect_candidates_applies_override_globs() {
        let dir = TempDir::new().unwrap();
        write(dir.path().join("src/lib.rs").as_path(), "pub fn lib() {}\n");
        write(
            dir.path().join("tests/lib.rs").as_path(),
            "fn test_lib() {}\n",
        );

        let input = StructuralSearchInput {
            language: StructuralLanguage::Rust,
            query: StructuralQuery::Kind {
                kind: "function_item".to_string(),
            },
            path: ".".to_string(),
            globs: vec!["src/*.rs".to_string()],
            limit: 20,
            include_meta: false,
            strictness: None,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        };

        let candidates = collect_candidates(dir.path(), &input).unwrap();

        assert_eq!(candidates, vec!["src/lib.rs"]);
    }
}
