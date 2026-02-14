//! MCP tool implementations.

mod analysis;
mod content;
mod index;
mod search;

// analysis
pub use analysis::{
    execute_refs, execute_stats, IndexSize, Reference, RefsInput, RefsOutput, StatsInput,
    StatsOutput,
};

// content
pub use content::{
    execute_context, execute_get, execute_outline, execute_toc, ContextInput, ContextOutput,
    GetInput, GetOutput, OutlineInput, OutlineOutput, Symbol, TocInput, TocOutput,
};

// index
pub use index::{
    execute_diff, execute_index, DiffHunk, DiffInput, DiffOutput, DiffStats, IndexInput,
    IndexOutput,
};

// search
pub use search::{
    execute_search, MatchSnippetOutput, SearchInput, SearchMode, SearchOutput, SearchResultItem,
};
