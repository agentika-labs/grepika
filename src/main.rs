//! grepika: Token-efficient MCP server for code search.
//!
//! Usage:
//!   grepika --mcp --root <path>   # Start MCP server (single workspace)
//!   grepika --mcp                 # Start MCP server (global mode, LLM calls add_workspace)
//!   grepika search <query>        # CLI search mode
//!   grepika index                 # Index the codebase

use grepika::server::GrepikaServer;
use clap::{Parser, Subcommand};
use rmcp::ServiceExt;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "grepika")]
#[command(about = "Token-efficient MCP server for code search")]
#[command(version)]
struct Cli {
    /// Run as MCP server (stdin/stdout JSON-RPC)
    #[arg(long)]
    mcp: bool,

    /// Root directory to search (omit for global mode where LLM calls add_workspace)
    #[arg(long)]
    root: Option<PathBuf>,

    /// Database path (default: ~/.cache/grepika/<hash>.db)
    #[arg(long)]
    db: Option<PathBuf>,

    /// File to write profiler logs (requires --features profiling)
    #[arg(long)]
    log_file: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Search for code patterns
    Search {
        /// Search query
        query: String,

        /// Maximum results
        #[arg(short, long, default_value = "20")]
        limit: usize,

        /// Search mode: combined, fts, or grep
        #[arg(short, long, default_value = "combined")]
        mode: String,
    },

    /// Index the codebase
    Index {
        /// Force full re-index
        #[arg(short, long)]
        force: bool,
    },

    /// Get file content
    Get {
        /// File path
        path: String,

        /// Start line
        #[arg(short, long, default_value = "1")]
        start: usize,

        /// End line (0 = end of file)
        #[arg(short, long, default_value = "0")]
        end: usize,
    },

    /// Get codebase statistics
    Stats {
        /// Show detailed breakdown
        #[arg(short, long)]
        detailed: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // CRITICAL: Log to stderr only (stdout is JSON-RPC for MCP)
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("grepika=info".parse()?))
        .with_writer(std::io::stderr)
        .init();

    // Initialize profiling log file (if profiling feature enabled)
    #[cfg(feature = "profiling")]
    grepika::server::init_profiling(cli.log_file.as_deref());

    #[cfg(not(feature = "profiling"))]
    if cli.log_file.is_some() {
        eprintln!("warning: --log-file has no effect without --features profiling");
    }

    if cli.mcp {
        // MCP server mode
        match cli.root {
            Some(root) => {
                // Explicit --root: backward compatible single-workspace mode
                let root = root.canonicalize().unwrap_or(root);
                run_mcp_server(root, cli.db).await
            }
            None => {
                // Global mode: start empty, LLM calls add_workspace
                run_mcp_server_global(cli.db).await
            }
        }
    } else if let Some(cmd) = cli.command {
        // CLI subcommands require --root
        let root = cli.root.unwrap_or_else(|| PathBuf::from("."));
        let root = root.canonicalize().unwrap_or(root);
        run_cli(root, cli.db, cmd).await
    } else {
        // Default: show help
        eprintln!("Use --mcp to start MCP server, or a subcommand for CLI mode.");
        eprintln!("Run with --help for more information.");
        std::process::exit(1);
    }
}

async fn run_mcp_server(root: PathBuf, db: Option<PathBuf>) -> anyhow::Result<()> {
    tracing::info!("Starting MCP server for root: {}", root.display());

    let server = GrepikaServer::new(root, db)?;

    // Run the MCP server on stdin/stdout
    let service = server.serve(rmcp::transport::io::stdio()).await?;
    service.waiting().await?;

    Ok(())
}

async fn run_mcp_server_global(db: Option<PathBuf>) -> anyhow::Result<()> {
    tracing::info!("Starting MCP server in global mode (no workspace pre-loaded)");

    let server = GrepikaServer::new_empty(db);

    let service = server.serve(rmcp::transport::io::stdio()).await?;
    service.waiting().await?;

    Ok(())
}

async fn run_cli(root: PathBuf, db: Option<PathBuf>, cmd: Commands) -> anyhow::Result<()> {
    use grepika::db::Database;
    use grepika::services::{Indexer, SearchService, TrigramIndex};
    use std::sync::Arc;
    use std::sync::RwLock;

    // Initialize database - use global cache location by default
    let db_path = db.unwrap_or_else(|| grepika::default_db_path(&root));
    std::fs::create_dir_all(db_path.parent().unwrap())?;
    let database = Arc::new(Database::open(&db_path)?);

    // Load trigram index from database (if previously persisted)
    let trigram = match database.load_all_trigrams() {
        Ok(entries) if !entries.is_empty() => {
            tracing::info!("Loaded {} trigrams from database", entries.len());
            Arc::new(RwLock::new(TrigramIndex::from_db_entries(entries)))
        }
        Ok(_) => Arc::new(RwLock::new(TrigramIndex::new())),
        Err(_) => Arc::new(RwLock::new(TrigramIndex::new())),
    };

    // Initialize services
    let search = Arc::new(SearchService::new(Arc::clone(&database), root.clone())?);
    let indexer = Indexer::new(Arc::clone(&database), Arc::clone(&trigram), root);

    match cmd {
        Commands::Search { query, limit, mode } => {
            let mode: grepika::tools::SearchMode = mode
                .parse()
                .map_err(|e: String| anyhow::anyhow!(e))?;
            let input = grepika::tools::SearchInput { query, limit, mode };
            let result = grepika::tools::execute_search(&search, input)
                .map_err(|e| anyhow::anyhow!(e))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }

        Commands::Index { force } => {
            let input = grepika::tools::IndexInput { force };
            let result = grepika::tools::execute_index(&indexer, input)
                .map_err(|e| anyhow::anyhow!(e))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }

        Commands::Get { path, start, end } => {
            let input = grepika::tools::GetInput {
                path,
                start_line: start,
                end_line: end,
            };
            let result = grepika::tools::execute_get(&search, input)
                .map_err(|e| anyhow::anyhow!(e))?;
            println!("{}", result.content);
        }

        Commands::Stats { detailed } => {
            let input = grepika::tools::StatsInput { detailed };
            let result = grepika::tools::execute_stats(&search, &indexer, input)
                .map_err(|e| anyhow::anyhow!(e))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }

    Ok(())
}
