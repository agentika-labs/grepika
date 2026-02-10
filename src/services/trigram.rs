//! Trigram index for fast substring search.
//!
//! Trigrams enable finding files containing any substring,
//! not just word boundaries like FTS. For example, searching
//! "auth" finds files with "authentication", "oauth", etc.

use crate::types::{FileId, Trigram};
use ahash::{AHashMap, AHashSet};
use roaring::RoaringBitmap;

/// In-memory trigram index using RoaringBitmaps.
///
/// Each trigram maps to a bitmap of file IDs containing it.
/// Substring search ANDs all trigram bitmaps together.
///
/// Tracks which trigrams were modified since last persistence,
/// enabling incremental saves that only write changed entries.
///
/// Thread-safe (Send + Sync) when wrapped in appropriate synchronization
/// primitives (e.g., `Arc<RwLock<TrigramIndex>>`).
pub struct TrigramIndex {
    /// Trigram -> FileIds containing this trigram
    index: AHashMap<Trigram, RoaringBitmap>,
    /// Trigrams modified since last `take_dirty_entries()` call
    dirty: AHashSet<Trigram>,
}

impl Default for TrigramIndex {
    fn default() -> Self {
        Self {
            index: AHashMap::new(),
            dirty: AHashSet::new(),
        }
    }
}

/// Upserts and deletes for incremental trigram persistence.
pub type DirtyEntries = (Vec<(Vec<u8>, Vec<u8>)>, Vec<Vec<u8>>);

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
            self.dirty.insert(trigram);
        }
    }

    /// Removes a file from the index.
    ///
    /// O(total_trigrams) — iterates all trigram bitmaps to remove the file ID.
    /// Acceptable for few deletions per cycle; consider a reverse index
    /// (FileId → Set<Trigram>) if bulk deletions become common.
    pub fn remove_file(&mut self, file_id: FileId) {
        let id = file_id.as_u32();
        for (trigram, bitmap) in &mut self.index {
            if bitmap.remove(id) {
                self.dirty.insert(*trigram);
            }
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

        // Collect bitmaps and check for missing trigrams
        let mut bitmaps: Vec<&RoaringBitmap> = Vec::with_capacity(trigrams.len());
        for trigram in &trigrams {
            if let Some(bitmap) = self.index.get(trigram) {
                bitmaps.push(bitmap);
            } else {
                return Some(RoaringBitmap::new()); // No files have this trigram
            }
        }

        // Start with smallest bitmap (P9: cheapest clone, fastest AND chain)
        bitmaps.sort_by_key(|b| b.len());
        let mut result = bitmaps[0].clone();

        for bitmap in &bitmaps[1..] {
            result &= *bitmap;
            if result.is_empty() {
                break; // Early exit: no files match all trigrams
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

    /// Clears the index and dirty set.
    pub fn clear(&mut self) {
        self.index.clear();
        self.dirty.clear();
    }

    /// Returns the number of dirty (modified) trigrams since last persistence.
    #[must_use]
    pub fn dirty_count(&self) -> usize {
        self.dirty.len()
    }

    /// Takes dirty entries for incremental persistence.
    ///
    /// Returns `(upserts, deletes)`:
    /// - `upserts`: trigrams with non-empty bitmaps that need INSERT OR REPLACE
    /// - `deletes`: trigram keys whose bitmaps are now empty (need DELETE)
    ///
    /// Clears the dirty set after taking entries.
    pub fn take_dirty_entries(&mut self) -> DirtyEntries {
        let mut upserts = Vec::with_capacity(self.dirty.len());
        let mut deletes = Vec::new();

        for trigram in self.dirty.drain() {
            match self.index.get(&trigram) {
                Some(bitmap) if !bitmap.is_empty() => {
                    upserts.push((trigram.as_bytes().to_vec(), Self::bitmap_to_bytes(bitmap)));
                }
                _ => {
                    // Bitmap is empty or missing — delete from DB
                    deletes.push(trigram.as_bytes().to_vec());
                    // Also clean up empty bitmaps from memory
                    self.index.remove(&trigram);
                }
            }
        }

        (upserts, deletes)
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

    /// Serializes the entire index to database-compatible entries.
    ///
    /// Each entry is a tuple of (trigram_bytes, serialized_bitmap).
    /// This is used to persist the index to the database.
    #[must_use]
    pub fn to_db_entries(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.index
            .iter()
            .map(|(trigram, bitmap)| (trigram.as_bytes().to_vec(), Self::bitmap_to_bytes(bitmap)))
            .collect()
    }

    /// Loads the index from database entries.
    ///
    /// Takes an iterator of (trigram_bytes, serialized_bitmap) tuples.
    /// Invalid entries are silently skipped.
    /// The dirty set starts empty since loaded entries are already persisted.
    pub fn from_db_entries<I>(entries: I) -> Self
    where
        I: IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
    {
        let mut index = AHashMap::new();

        for (trigram_bytes, bitmap_bytes) in entries {
            // Trigram must be exactly 3 bytes
            if trigram_bytes.len() != 3 {
                continue;
            }

            let trigram = Trigram([trigram_bytes[0], trigram_bytes[1], trigram_bytes[2]]);

            if let Some(bitmap) = Self::bitmap_from_bytes(&bitmap_bytes) {
                index.insert(trigram, bitmap);
            }
        }

        Self {
            index,
            dirty: AHashSet::new(),
        }
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

// Compile-time assertion for thread safety.
#[cfg(test)]
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<TrigramIndex>();
};

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

    #[test]
    fn test_dirty_tracking_add() {
        let mut index = TrigramIndex::new();
        assert_eq!(index.dirty_count(), 0);

        index.add_file(FileId::new(1), "hello");
        assert!(index.dirty_count() > 0);

        let (upserts, deletes) = index.take_dirty_entries();
        assert!(!upserts.is_empty());
        assert!(deletes.is_empty());
        // After taking, dirty set is cleared
        assert_eq!(index.dirty_count(), 0);
    }

    #[test]
    fn test_dirty_tracking_remove_produces_deletes() {
        let mut index = TrigramIndex::new();
        index.add_file(FileId::new(1), "xyz");
        // Clear dirty set (simulating a persistence cycle)
        let _ = index.take_dirty_entries();
        assert_eq!(index.dirty_count(), 0);

        // Remove the only file — bitmaps become empty
        index.remove_file(FileId::new(1));
        assert!(index.dirty_count() > 0);

        let (upserts, deletes) = index.take_dirty_entries();
        // All trigrams from "xyz" had only file 1, so they're now empty -> deletes
        assert!(!deletes.is_empty());
        // No non-empty bitmaps to upsert
        assert!(upserts.is_empty());
    }

    #[test]
    fn test_dirty_tracking_incremental() {
        let mut index = TrigramIndex::new();
        index.add_file(FileId::new(1), "authentication");
        // Simulate persistence
        let _ = index.take_dirty_entries();
        assert_eq!(index.dirty_count(), 0);

        // Add another file — only new/changed trigrams should be dirty
        index.add_file(FileId::new(2), "authorization");
        let dirty_count = index.dirty_count();
        let total_count = index.trigram_count();
        // Dirty trigrams should be a subset of total
        assert!(dirty_count <= total_count);
        assert!(dirty_count > 0);
    }

    #[test]
    fn test_from_db_entries_starts_clean() {
        let mut index = TrigramIndex::new();
        index.add_file(FileId::new(1), "hello world");
        let entries = index.to_db_entries();

        let loaded = TrigramIndex::from_db_entries(entries);
        // Loaded from DB — nothing is dirty
        assert_eq!(loaded.dirty_count(), 0);
        // But the index has data
        assert!(loaded.trigram_count() > 0);
    }
}
