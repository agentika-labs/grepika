//! agentika-grep: Token-efficient MCP server for code search.
//!
//! Usage:
//!   agentika-grep --mcp --root <path>   # Start MCP server
//!   agentika-grep search <query>        # CLI search mode
//!   agentika-grep index                 # Index the codebase

use agentika_grep::server::AgentikaGrepServer;
use clap::{Parser, Subcommand};
use rmcp::ServiceExt;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "agentika-grep")]
#[command(about = "Token-efficient MCP server for code search")]
#[command(version)]
struct Cli {
    /// Run as MCP server (stdin/stdout JSON-RPC)
    #[arg(long)]
    mcp: bool,

    /// Root directory to search
    #[arg(long, default_value = ".")]
    root: PathBuf,

    /// Database path (default: <root>/.agentika-grep/index.db)
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
        .with_env_filter(EnvFilter::from_default_env().add_directive("agentika_grep=info".parse()?))
        .with_writer(std::io::stderr)
        .init();

    // Resolve root path
    let root = cli.root.canonicalize().unwrap_or(cli.root);

    // Initialize profiling log file (if profiling feature enabled)
    #[cfg(feature = "profiling")]
    agentika_grep::server::init_profiling(cli.log_file.as_deref());

    if cli.mcp {
        // MCP server mode
        run_mcp_server(root, cli.db).await
    } else if let Some(cmd) = cli.command {
        // CLI mode
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

    let server = AgentikaGrepServer::new(root, db)?;

    // Run the MCP server on stdin/stdout
    let service = server.serve(rmcp::transport::io::stdio()).await?;
    service.waiting().await?;

    Ok(())
}

async fn run_cli(root: PathBuf, db: Option<PathBuf>, cmd: Commands) -> anyhow::Result<()> {
    use agentika_grep::db::Database;
    use agentika_grep::services::{Indexer, SearchService, TrigramIndex};
    use std::sync::Arc;
    use std::sync::RwLock;

    // Initialize database
    let db_path = db.unwrap_or_else(|| root.join(".agentika-grep").join("index.db"));
    std::fs::create_dir_all(db_path.parent().unwrap())?;
    let database = Arc::new(Database::open(&db_path)?);

    // Initialize services
    let trigram = Arc::new(RwLock::new(TrigramIndex::new()));
    let search = Arc::new(SearchService::new(Arc::clone(&database), root.clone())?);
    let indexer = Indexer::new(Arc::clone(&database), trigram, root.clone());

    match cmd {
        Commands::Search { query, limit, mode } => {
            let input = agentika_grep::tools::SearchInput { query, limit, mode };
            let result = agentika_grep::tools::execute_search(&search, input)
                .map_err(|e| anyhow::anyhow!(e))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }

        Commands::Index { force } => {
            let input = agentika_grep::tools::IndexInput { force };
            let result = agentika_grep::tools::execute_index(&indexer, input)
                .map_err(|e| anyhow::anyhow!(e))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }

        Commands::Get { path, start, end } => {
            let input = agentika_grep::tools::GetInput {
                path,
                start_line: start,
                end_line: end,
            };
            let result = agentika_grep::tools::execute_get(&search, input)
                .map_err(|e| anyhow::anyhow!(e))?;
            println!("{}", result.content);
        }

        Commands::Stats { detailed } => {
            let input = agentika_grep::tools::StatsInput { detailed };
            let result = agentika_grep::tools::execute_stats(&search, &indexer, input)
                .map_err(|e| anyhow::anyhow!(e))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }

    Ok(())
}
