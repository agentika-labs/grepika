//! Core services for search, indexing, and file operations.

pub mod ast;
mod fts;
mod git_diff;
pub mod grep;
pub mod indexer;
pub(crate) mod ngram;
mod regex_literals;
mod search;
pub mod semantic;
pub mod structural;
mod trigram;

pub use fts::FtsService;
pub use grep::{GrepMatch, GrepService};
pub use indexer::Indexer;
pub use search::{MatchSnippet, SearchResult as SearchHit, SearchService, SearchSources};
pub use trigram::TrigramIndex;
