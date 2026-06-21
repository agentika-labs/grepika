//! MCP tool implementations.

mod analysis;
mod content;
mod graph;
mod index;
mod search;
mod structural;

// graph
pub use graph::{execute_graph, GraphInput, GraphOutput, SymbolHit, MAX_DEPTH};

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

// structural
pub use structural::{
    execute_structural_search, execute_structural_search_with_backend, StructuralLanguage,
    StructuralQuery, StructuralSearchHit, StructuralSearchInput, StructuralSearchOutput,
    StructuralStrictness, STRUCTURAL_DEFAULT_LIMIT, STRUCTURAL_DEFAULT_TIMEOUT_MS,
};
