//! Structural search MCP tool.

use crate::services::structural::{AstGrepNativeBackend, StructuralSearchBackend};
use crate::services::SearchService;

pub use crate::services::structural::{
    StructuralLanguage, StructuralQuery, StructuralSearchHit, StructuralSearchInput,
    StructuralSearchOutput, StructuralStrictness, DEFAULT_LIMIT as STRUCTURAL_DEFAULT_LIMIT,
    DEFAULT_TIMEOUT_MS as STRUCTURAL_DEFAULT_TIMEOUT_MS,
};

/// Executes structural search using the default native ast-grep backend.
///
/// # Errors
///
/// Returns a `ServerError` if validation, candidate collection, parsing, or
/// matching fails.
pub fn execute_structural_search(
    service: &SearchService,
    input: StructuralSearchInput,
) -> crate::error::Result<StructuralSearchOutput> {
    execute_structural_search_with_backend(service.root(), &AstGrepNativeBackend, input)
}

/// Executes structural search with an injected backend.
///
/// This keeps tests independent from the concrete ast-grep backend.
///
/// # Errors
///
/// Returns a `ServerError` from the selected backend.
pub fn execute_structural_search_with_backend<B: StructuralSearchBackend>(
    root: &std::path::Path,
    backend: &B,
    input: StructuralSearchInput,
) -> crate::error::Result<StructuralSearchOutput> {
    backend.search(root, input)
}
