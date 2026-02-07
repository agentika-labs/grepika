//! Core services for search, indexing, and file operations.

mod fts;
pub mod grep;
pub mod indexer;
mod search;
mod trigram;

pub use fts::FtsService;
pub use grep::GrepService;
pub use indexer::Indexer;
pub use search::{MatchSnippet, SearchResult as SearchHit, SearchService, SearchSources};
pub use trigram::TrigramIndex;
