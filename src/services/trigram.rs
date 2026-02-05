//! Trigram index for fast substring search.
//!
//! Trigrams enable finding files containing any substring,
//! not just word boundaries like FTS. For example, searching
//! "auth" finds files with "authentication", "oauth", etc.

use crate::types::{FileId, Trigram};
use ahash::AHashMap;
use roaring::RoaringBitmap;

/// In-memory trigram index using RoaringBitmaps.
///
/// Each trigram maps to a bitmap of file IDs containing it.
/// Substring search ANDs all trigram bitmaps together.
#[derive(Default)]
pub struct TrigramIndex {
    /// Trigram -> FileIds containing this trigram
    index: AHashMap<Trigram, RoaringBitmap>,
}

impl TrigramIndex {
    /// Creates an empty trigram index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file's content to the index.
    pub fn add_file(&mut self, file_id: FileId, content: &str) {
        for trigram in Trigram::extract(content) {
            self.index
                .entry(trigram)
                .or_default()
                .insert(file_id.as_u32());
        }
    }

    /// Removes a file from the index.
    pub fn remove_file(&mut self, file_id: FileId) {
        for bitmap in self.index.values_mut() {
            bitmap.remove(file_id.as_u32());
        }
    }

    /// Searches for files containing the query substring.
    ///
    /// Returns file IDs that contain ALL trigrams from the query.
    /// For queries shorter than 3 characters, returns None (no filtering).
    pub fn search(&self, query: &str) -> Option<RoaringBitmap> {
        let trigrams: Vec<_> = Trigram::extract(query).collect();

        if trigrams.is_empty() {
            return None; // Query too short for trigram filtering
        }

        // Start with first trigram's files
        let mut result = self.index.get(&trigrams[0])?.clone();

        // AND with remaining trigrams
        for trigram in &trigrams[1..] {
            if let Some(bitmap) = self.index.get(trigram) {
                result &= bitmap;
            } else {
                return Some(RoaringBitmap::new()); // No files have this trigram
            }
        }

        Some(result)
    }

    /// Returns the number of unique trigrams indexed.
    #[must_use]
    pub fn trigram_count(&self) -> usize {
        self.index.len()
    }

    /// Returns total file references across all trigrams.
    #[must_use]
    pub fn total_refs(&self) -> u64 {
        self.index.values().map(|b| b.len()).sum()
    }

    /// Clears the index.
    pub fn clear(&mut self) {
        self.index.clear();
    }

    /// Serializes a bitmap to bytes for database storage.
    ///
    /// # Panics
    ///
    /// Panics if serialization fails, which should not happen with a valid `RoaringBitmap`
    /// and a `Vec<u8>` writer (infallible for in-memory writes).
    #[must_use]
    pub fn bitmap_to_bytes(bitmap: &RoaringBitmap) -> Vec<u8> {
        let mut bytes = Vec::new();
        // Writing to Vec<u8> is infallible - unwrap is safe here
        bitmap
            .serialize_into(&mut bytes)
            .expect("RoaringBitmap serialization to Vec<u8> is infallible");
        bytes
    }

    /// Deserializes a bitmap from bytes.
    #[must_use]
    pub fn bitmap_from_bytes(bytes: &[u8]) -> Option<RoaringBitmap> {
        RoaringBitmap::deserialize_from(bytes).ok()
    }

    /// Gets the bitmap for a specific trigram.
    #[must_use]
    pub fn get_trigram(&self, trigram: &Trigram) -> Option<&RoaringBitmap> {
        self.index.get(trigram)
    }

    /// Sets a trigram's bitmap directly (for loading from DB).
    pub fn set_trigram(&mut self, trigram: Trigram, bitmap: RoaringBitmap) {
        self.index.insert(trigram, bitmap);
    }
}

impl std::fmt::Debug for TrigramIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrigramIndex")
            .field("trigram_count", &self.trigram_count())
            .field("total_refs", &self.total_refs())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_search() {
        let mut index = TrigramIndex::new();
        index.add_file(FileId::new(1), "authentication");
        index.add_file(FileId::new(2), "authorization");
        index.add_file(FileId::new(3), "oauth provider");

        // "auth" matches files 1, 2, AND 3 (oauth contains "auth")
        let results = index.search("auth").unwrap();
        assert!(results.contains(1));
        assert!(results.contains(2));
        assert!(results.contains(3)); // "oauth" contains "auth"

        // "oauth" should only match file 3
        let results = index.search("oauth").unwrap();
        assert!(results.contains(3));
        assert!(!results.contains(1));
    }

    #[test]
    fn test_short_query() {
        let mut index = TrigramIndex::new();
        index.add_file(FileId::new(1), "test content");

        // Queries < 3 chars return None (no filtering)
        assert!(index.search("te").is_none());
        assert!(index.search("t").is_none());
    }

    #[test]
    fn test_no_match() {
        let mut index = TrigramIndex::new();
        index.add_file(FileId::new(1), "hello world");

        // "xyz" has no matching trigrams, returns None or empty
        let results = index.search("xyz");
        assert!(results.is_none() || results.unwrap().is_empty());
    }

    #[test]
    fn test_bitmap_serialization() {
        let mut bitmap = RoaringBitmap::new();
        bitmap.insert(1);
        bitmap.insert(100);
        bitmap.insert(1000);

        let bytes = TrigramIndex::bitmap_to_bytes(&bitmap);
        let restored = TrigramIndex::bitmap_from_bytes(&bytes).unwrap();

        assert_eq!(bitmap, restored);
    }

    #[test]
    fn test_remove_file() {
        let mut index = TrigramIndex::new();
        index.add_file(FileId::new(1), "authentication");
        index.add_file(FileId::new(2), "authorization");

        index.remove_file(FileId::new(1));

        let results = index.search("auth").unwrap();
        assert!(!results.contains(1));
        assert!(results.contains(2));
    }
}
