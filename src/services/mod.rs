//! Core services for search, indexing, and file operations.

mod fts;
mod grep;
mod indexer;
mod search;
mod trigram;

pub use fts::FtsService;
pub use grep::GrepService;
pub use indexer::Indexer;
pub use search::{SearchResult as SearchHit, SearchService};
pub use trigram::TrigramIndex;
