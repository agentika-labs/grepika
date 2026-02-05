//! MCP server implementation using rmcp.

use crate::db::Database;
use crate::services::{Indexer, SearchService, TrigramIndex};
use crate::tools;
use rmcp::model::{CallToolResult, Content, ServerCapabilities, ServerInfo};
use rmcp::{schemars, tool, ServerHandler};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;

// ============ PROFILING ENABLED ============
#[cfg(feature = "profiling")]
mod profiling {
    use std::fs::{File, OpenOptions};
    use std::io::Write;
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::OnceLock;

    static LOG_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();

    /// Initialize profiling with optional log file path.
    /// If path is None, logs go to stderr (default).
    /// If path is Some, logs are appended to the specified file.
    pub fn init(path: Option<&Path>) {
        LOG_FILE.get_or_init(|| {
            path.and_then(|p| {
                // Create parent directories if needed
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)
                    .ok()
                    .map(Mutex::new)
            })
        });
    }

    /// Log a profiling message to file or stderr.
    pub fn log(msg: &str) {
        if let Some(Some(file)) = LOG_FILE.get() {
            if let Ok(mut f) = file.lock() {
                let _ = writeln!(f, "{}", msg);
                let _ = f.flush();
            }
        } else {
            eprintln!("{}", msg);
        }
    }

    /// Gets current memory usage in MB (macOS/Linux only).
    pub fn get_memory_mb() -> f64 {
        use std::process::Command;
        Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(|kb| kb as f64 / 1024.0)
            .unwrap_or(0.0)
    }
}

/// Initialize profiling log file (public API).
#[cfg(feature = "profiling")]
pub fn init_profiling(path: Option<&std::path::Path>) {
    profiling::init(path);
}

/// Helper to run a blocking tool operation and return structured MCP results.
///
/// Uses `spawn_blocking()` for CPU-bound work and returns either:
/// - `CallToolResult::success()` with JSON content for success
/// - `CallToolResult::error()` with error details for tool errors
/// - `rmcp::Error::internal_error()` for panics/JoinErrors
///
/// When the `profiling` feature is enabled, logs timing, memory usage, and
/// estimated token count to stderr.
#[cfg(feature = "profiling")]
async fn run_tool<T, E, F>(name: &'static str, f: F) -> Result<CallToolResult, rmcp::Error>
where
    T: Serialize + Send + 'static,
    E: std::fmt::Display + Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    let start = std::time::Instant::now();
    let mem_before = profiling::get_memory_mb();

    let result = tokio::task::spawn_blocking(f).await;

    let elapsed = start.elapsed();
    let mem_after = profiling::get_memory_mb();
    let mem_delta = mem_after - mem_before;

    match result {
        Ok(Ok(output)) => {
            let json = serde_json::to_string_pretty(&output)
                .map_err(|e| rmcp::Error::internal_error(e.to_string(), None))?;

            // Token estimation (~4 bytes per token for code/text)
            let bytes = json.len();
            let tokens = (bytes + 2) / 4; // Rounded division

            profiling::log(&format!(
                "[{}] {:?} | mem: {:.1}MB ({:+.1}MB) | ~{} tokens ({:.1}KB)",
                name, elapsed, mem_after, mem_delta, tokens, bytes as f64 / 1024.0
            ));

            Ok(CallToolResult::success(vec![Content::text(json)]))
        }
        Ok(Err(e)) => {
            profiling::log(&format!(
                "[{}] {:?} | mem: {:.1}MB ({:+.1}MB) | ERROR",
                name, elapsed, mem_after, mem_delta
            ));
            Ok(CallToolResult::error(vec![Content::text(e.to_string())]))
        }
        Err(e) => Err(rmcp::Error::internal_error(e.to_string(), None)),
    }
}

// ============ PROFILING DISABLED (production) ============
#[cfg(not(feature = "profiling"))]
async fn run_tool<T, E, F>(name: &'static str, f: F) -> Result<CallToolResult, rmcp::Error>
where
    T: Serialize + Send + 'static,
    E: std::fmt::Display + Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    let _ = name; // Suppress unused warning
    let result = tokio::task::spawn_blocking(f).await;

    match result {
        Ok(Ok(output)) => {
            let json = serde_json::to_string_pretty(&output)
                .map_err(|e| rmcp::Error::internal_error(e.to_string(), None))?;
            Ok(CallToolResult::success(vec![Content::text(json)]))
        }
        Ok(Err(e)) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        Err(e) => Err(rmcp::Error::internal_error(e.to_string(), None)),
    }
}

/// MCP Server for code search.
#[derive(Clone)]
pub struct AgentikaGrepServer {
    search: Arc<SearchService>,
    indexer: Arc<Indexer>,
    #[allow(dead_code)]
    root: PathBuf,
}

impl AgentikaGrepServer {
    /// Creates a new server instance.
    pub fn new(root: PathBuf, db_path: Option<PathBuf>) -> Result<Self, crate::ServerError> {
        // Initialize database
        let db = if let Some(path) = db_path {
            Arc::new(Database::open(&path)?)
        } else {
            // Default to .agentika-grep/index.db in root
            let db_dir = root.join(".agentika-grep");
            std::fs::create_dir_all(&db_dir)?;
            Arc::new(Database::open(&db_dir.join("index.db"))?)
        };

        // Initialize services
        let trigram = Arc::new(RwLock::new(TrigramIndex::new()));
        let search = Arc::new(SearchService::new(Arc::clone(&db), root.clone())?);
        let indexer = Arc::new(Indexer::new(
            Arc::clone(&db),
            Arc::clone(&trigram),
            root.clone(),
        ));

        Ok(Self {
            search,
            indexer,
            root,
        })
    }
}

// Tool implementations using rmcp macros
#[tool(tool_box)]
impl AgentikaGrepServer {
    /// Search for code patterns across the codebase.
    #[tool(description = "Search for code patterns. Supports regex and natural language queries.")]
    async fn search(
        &self,
        #[tool(param)]
        #[schemars(description = "Search query (regex or natural language)")]
        query: String,
        #[tool(param)]
        #[schemars(description = "Maximum results (default: 20)")]
        limit: Option<usize>,
        #[tool(param)]
        #[schemars(description = "Search mode: combined, fts, or grep (default: combined)")]
        mode: Option<String>,
    ) -> Result<CallToolResult, rmcp::Error> {
        let input = tools::SearchInput {
            query,
            limit: limit.unwrap_or(20),
            mode: mode.unwrap_or_else(|| "combined".to_string()),
        };
        let search = Arc::clone(&self.search);
        run_tool("search", move || tools::execute_search(&search, input)).await
    }

    /// Find files most relevant to a topic or concept.
    #[tool(
        description = "Find files most relevant to a topic. Uses combined search for best results."
    )]
    async fn relevant(
        &self,
        #[tool(param)]
        #[schemars(description = "Topic or concept to search for")]
        topic: String,
        #[tool(param)]
        #[schemars(description = "Maximum files (default: 10)")]
        limit: Option<usize>,
    ) -> Result<CallToolResult, rmcp::Error> {
        let input = tools::RelevantInput {
            topic,
            limit: limit.unwrap_or(10),
        };
        let search = Arc::clone(&self.search);
        run_tool("relevant", move || tools::execute_relevant(&search, input)).await
    }

    /// Get file content with optional line range.
    #[tool(description = "Get file content. Supports line range selection.")]
    async fn get(
        &self,
        #[tool(param)]
        #[schemars(description = "File path relative to root")]
        path: String,
        #[tool(param)]
        #[schemars(description = "Starting line (1-indexed, default: 1)")]
        start_line: Option<usize>,
        #[tool(param)]
        #[schemars(description = "Ending line (0 = end of file)")]
        end_line: Option<usize>,
    ) -> Result<CallToolResult, rmcp::Error> {
        let input = tools::GetInput {
            path,
            start_line: start_line.unwrap_or(1),
            end_line: end_line.unwrap_or(0),
        };
        let search = Arc::clone(&self.search);
        run_tool("get", move || tools::execute_get(&search, input)).await
    }

    /// Get file outline showing functions, classes, and other symbols.
    #[tool(description = "Extract file structure (functions, classes, structs, etc.)")]
    async fn outline(
        &self,
        #[tool(param)]
        #[schemars(description = "File path relative to root")]
        path: String,
    ) -> Result<CallToolResult, rmcp::Error> {
        let input = tools::OutlineInput { path };
        let search = Arc::clone(&self.search);
        run_tool("outline", move || tools::execute_outline(&search, input)).await
    }

    /// Get directory table of contents.
    #[tool(description = "Get directory tree structure")]
    async fn toc(
        &self,
        #[tool(param)]
        #[schemars(description = "Directory path (default: root)")]
        path: Option<String>,
        #[tool(param)]
        #[schemars(description = "Maximum depth (default: 3)")]
        depth: Option<usize>,
    ) -> Result<CallToolResult, rmcp::Error> {
        let input = tools::TocInput {
            path: path.unwrap_or_else(|| ".".to_string()),
            depth: depth.unwrap_or(3),
        };
        let search = Arc::clone(&self.search);
        run_tool("toc", move || tools::execute_toc(&search, input)).await
    }

    /// Get context around a specific line.
    #[tool(description = "Get surrounding context for a line")]
    async fn context(
        &self,
        #[tool(param)]
        #[schemars(description = "File path")]
        path: String,
        #[tool(param)]
        #[schemars(description = "Center line number")]
        line: usize,
        #[tool(param)]
        #[schemars(description = "Lines of context before and after (default: 10)")]
        context_lines: Option<usize>,
    ) -> Result<CallToolResult, rmcp::Error> {
        let input = tools::ContextInput {
            path,
            line,
            context_lines: context_lines.unwrap_or(10),
        };
        let search = Arc::clone(&self.search);
        run_tool("context", move || tools::execute_context(&search, input)).await
    }

    /// Get index statistics.
    #[tool(description = "Get index statistics and file type breakdown")]
    async fn stats(
        &self,
        #[tool(param)]
        #[schemars(description = "Include detailed breakdown by file type")]
        detailed: Option<bool>,
    ) -> Result<CallToolResult, rmcp::Error> {
        let input = tools::StatsInput {
            detailed: detailed.unwrap_or(false),
        };
        let search = Arc::clone(&self.search);
        let indexer = Arc::clone(&self.indexer);
        run_tool("stats", move || tools::execute_stats(&search, &indexer, input)).await
    }

    /// Find files related to a given file.
    #[tool(description = "Find files related to a source file by shared symbols")]
    async fn related(
        &self,
        #[tool(param)]
        #[schemars(description = "Source file path")]
        path: String,
        #[tool(param)]
        #[schemars(description = "Maximum related files (default: 10)")]
        limit: Option<usize>,
    ) -> Result<CallToolResult, rmcp::Error> {
        let input = tools::RelatedInput {
            path,
            limit: limit.unwrap_or(10),
        };
        let search = Arc::clone(&self.search);
        run_tool("related", move || tools::execute_related(&search, input)).await
    }

    /// Find references to a symbol.
    #[tool(description = "Find all references to a symbol/identifier")]
    async fn refs(
        &self,
        #[tool(param)]
        #[schemars(description = "Symbol/identifier to find")]
        symbol: String,
        #[tool(param)]
        #[schemars(description = "Maximum references (default: 50)")]
        limit: Option<usize>,
    ) -> Result<CallToolResult, rmcp::Error> {
        let input = tools::RefsInput {
            symbol,
            limit: limit.unwrap_or(50),
        };
        let search = Arc::clone(&self.search);
        run_tool("refs", move || tools::execute_refs(&search, input)).await
    }

    /// Update the search index.
    #[tool(description = "Update the search index (incremental by default)")]
    async fn index(
        &self,
        #[tool(param)]
        #[schemars(description = "Force full re-index")]
        force: Option<bool>,
    ) -> Result<CallToolResult, rmcp::Error> {
        let input = tools::IndexInput {
            force: force.unwrap_or(false),
        };
        let indexer = Arc::clone(&self.indexer);
        run_tool("index", move || tools::execute_index(&indexer, input)).await
    }

    /// Compare two files.
    #[tool(description = "Show differences between two files")]
    async fn diff(
        &self,
        #[tool(param)]
        #[schemars(description = "First file path")]
        file1: String,
        #[tool(param)]
        #[schemars(description = "Second file path")]
        file2: String,
        #[tool(param)]
        #[schemars(description = "Context lines around changes (default: 3)")]
        context: Option<usize>,
    ) -> Result<CallToolResult, rmcp::Error> {
        let input = tools::DiffInput {
            file1,
            file2,
            context: context.unwrap_or(3),
        };
        let search = Arc::clone(&self.search);
        run_tool("diff", move || tools::execute_diff(&search, input)).await
    }
}

// Implement ServerHandler trait
#[tool(tool_box)]
impl ServerHandler for AgentikaGrepServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("Token-efficient code search server with trigram indexing".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
