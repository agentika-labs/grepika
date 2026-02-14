//! grepika: Token-efficient MCP server for code search.
//!
//! Usage:
//!   grepika --mcp --root <path>   # Start MCP server (single workspace)
//!   grepika --mcp                 # Start MCP server (global mode, LLM calls add_workspace)
//!   grepika search <query>        # CLI search mode
//!   grepika index                 # Index the codebase

use clap::{Parser, Subcommand, ValueEnum};
use grepika::fmt;
use grepika::server::GrepikaServer;
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

    /// Write profiling logs (timing, memory) to this file
    #[arg(long)]
    log_file: Option<PathBuf>,

    /// Output as JSON (compact by default, use --pretty for indented)
    #[arg(long, global = true)]
    json: bool,

    /// Pretty-print JSON output (use with --json)
    #[arg(long, global = true)]
    pretty: bool,

    /// Color output: auto (default), always, never
    #[arg(long, global = true, default_value = "auto")]
    color: ColorChoice,

    #[command(subcommand)]
    command: Option<Commands>,
}

/// Controls when colored output is used.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
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

    /// Extract file structure (functions, classes, structs)
    Outline {
        /// File path
        path: String,
    },

    /// Show directory tree
    Toc {
        /// Directory path (default: current directory)
        #[arg(default_value = ".")]
        path: String,

        /// Maximum depth
        #[arg(short, long, default_value = "3")]
        depth: usize,
    },

    /// Get surrounding context for a line
    Context {
        /// File path
        path: String,

        /// Center line number
        line: usize,

        /// Lines of context before and after
        #[arg(short = 'C', long, default_value = "10")]
        context_lines: usize,
    },

    /// Find all references to a symbol
    Refs {
        /// Symbol/identifier to find
        symbol: String,

        /// Maximum references
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },

    /// Show differences between two files
    Diff {
        /// First file path
        file1: String,

        /// Second file path
        file2: String,

        /// Context lines around changes
        #[arg(short = 'C', long, default_value = "3")]
        context: usize,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
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

    // Initialize profiling (active only when --log-file is provided)
    grepika::profiling::init(cli.log_file.as_deref());

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
        run_cli(root, cli.db, cmd, cli.json, cli.pretty, cli.color).await
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

async fn run_cli(
    root: PathBuf,
    db: Option<PathBuf>,
    cmd: Commands,
    json: bool,
    pretty: bool,
    color: ColorChoice,
) -> anyhow::Result<()> {
    use grepika::db::Database;
    use grepika::services::{Indexer, SearchService, TrigramIndex};
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::RwLock;

    let use_color = match color {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            !json
                && std::io::IsTerminal::is_terminal(&std::io::stdout())
                && std::env::var_os("NO_COLOR").is_none()
        }
    };

    let mut out = std::io::stdout().lock();

    // Tier 0: No services needed
    if let Commands::Completions { shell } = cmd {
        let mut cli_cmd = <Cli as clap::CommandFactory>::command();
        clap_complete::generate(shell, &mut cli_cmd, "grepika", &mut out);
        return Ok(());
    }

    // Tier 1: Database + SearchService (no trigrams loaded)
    let db_path = db.unwrap_or_else(|| grepika::default_db_path(&root));
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let database = Arc::new(Database::open(&db_path)?);
    let search = Arc::new(SearchService::new(Arc::clone(&database), root.clone())?);

    // Tier 2: Load trigrams only for commands that need them
    let needs_trigram = matches!(
        cmd,
        Commands::Search { .. } | Commands::Index { .. } | Commands::Stats { .. }
    );
    let trigram = if needs_trigram {
        match database.load_all_trigrams() {
            Ok(entries) if !entries.is_empty() => {
                tracing::info!("Loaded {} trigrams from database", entries.len());
                Arc::new(RwLock::new(TrigramIndex::from_db_entries(entries)))
            }
            _ => Arc::new(RwLock::new(TrigramIndex::new())),
        }
    } else {
        Arc::new(RwLock::new(TrigramIndex::new()))
    };

    let indexer = Indexer::new(Arc::clone(&database), Arc::clone(&trigram), root);

    /// Outputs `result` as JSON (compact or pretty) and returns Ok.
    macro_rules! output_json {
        ($result:expr) => {
            if pretty {
                writeln!(out, "{}", serde_json::to_string_pretty(&$result)?)?;
            } else {
                writeln!(out, "{}", serde_json::to_string(&$result)?)?;
            }
        };
    }

    match cmd {
        Commands::Search { query, limit, mode } => {
            let mode: grepika::tools::SearchMode =
                mode.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            let input = grepika::tools::SearchInput { query, limit, mode };
            let result =
                grepika::tools::execute_search(&search, input).map_err(|e| anyhow::anyhow!(e))?;
            let empty = result.results.is_empty();
            if json {
                output_json!(result);
            } else {
                fmt::fmt_search(&mut out, &result, use_color)?;
            }
            if empty {
                std::process::exit(1);
            }
        }

        Commands::Index { force } => {
            let input = grepika::tools::IndexInput { force };
            let result = grepika::tools::execute_index(&indexer, input, None)
                .map_err(|e| anyhow::anyhow!(e))?;
            if json {
                output_json!(result);
            } else {
                fmt::fmt_index(&mut out, &result)?;
            }
        }

        Commands::Get { path, start, end } => {
            let input = grepika::tools::GetInput {
                path,
                start_line: start,
                end_line: end,
            };
            let result =
                grepika::tools::execute_get(&search, input).map_err(|e| anyhow::anyhow!(e))?;
            if json {
                output_json!(result);
            } else {
                fmt::fmt_get(&mut out, &result)?;
            }
        }

        Commands::Stats { detailed } => {
            let input = grepika::tools::StatsInput { detailed };
            let result = grepika::tools::execute_stats(&search, &indexer, input)
                .map_err(|e| anyhow::anyhow!(e))?;
            if json {
                output_json!(result);
            } else {
                fmt::fmt_stats(&mut out, &result, use_color)?;
            }
        }

        Commands::Outline { path } => {
            let input = grepika::tools::OutlineInput { path };
            let result =
                grepika::tools::execute_outline(&search, input).map_err(|e| anyhow::anyhow!(e))?;
            if json {
                output_json!(result);
            } else {
                fmt::fmt_outline(&mut out, &result, use_color)?;
            }
        }

        Commands::Toc { path, depth } => {
            let input = grepika::tools::TocInput { path, depth };
            let result =
                grepika::tools::execute_toc(&search, input).map_err(|e| anyhow::anyhow!(e))?;
            if json {
                output_json!(result);
            } else {
                fmt::fmt_toc(&mut out, &result)?;
            }
        }

        Commands::Context {
            path,
            line,
            context_lines,
        } => {
            let input = grepika::tools::ContextInput {
                path,
                line,
                context_lines,
            };
            let result =
                grepika::tools::execute_context(&search, input).map_err(|e| anyhow::anyhow!(e))?;
            if json {
                output_json!(result);
            } else {
                fmt::fmt_context(&mut out, &result, use_color)?;
            }
        }

        Commands::Refs { symbol, limit } => {
            let input = grepika::tools::RefsInput { symbol, limit };
            let result =
                grepika::tools::execute_refs(&search, input).map_err(|e| anyhow::anyhow!(e))?;
            let empty = result.references.is_empty();
            if json {
                output_json!(result);
            } else {
                fmt::fmt_refs(&mut out, &result, use_color)?;
            }
            if empty {
                std::process::exit(1);
            }
        }

        Commands::Diff {
            file1,
            file2,
            context,
        } => {
            let input = grepika::tools::DiffInput {
                file1,
                file2,
                context,
                max_lines: 0, // CLI: unlimited (user can pipe through head)
            };
            let result =
                grepika::tools::execute_diff(&search, input).map_err(|e| anyhow::anyhow!(e))?;
            if json {
                output_json!(result);
            } else {
                fmt::fmt_diff(&mut out, &result, use_color)?;
            }
        }

        Commands::Completions { .. } => unreachable!(),
    }

    Ok(())
}
