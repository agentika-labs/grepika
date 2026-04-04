//! N-gram index for fast substring search.
//!
//! Uses sparse frequency-weighted n-grams instead of dense 3-byte trigrams.
//! Variable-length n-grams produce more selective posting lists, reducing
//! the frequency of the 80% selectivity threshold skip.

use crate::services::ngram;
use crate::types::{FileId, NgramKey};
use ahash::{AHashMap, AHashSet};
use roaring::RoaringBitmap;

/// In-memory n-gram index using RoaringBitmaps.
///
/// Each n-gram key maps to a bitmap of file IDs containing it.
/// Substring search ANDs all n-gram bitmaps together.
///
/// Tracks which keys were modified since last persistence,
/// enabling incremental saves that only write changed entries.
///
/// Thread-safe (Send + Sync) when wrapped in appropriate synchronization
/// primitives (e.g., `Arc<RwLock<TrigramIndex>>`).
pub struct TrigramIndex {
    /// NgramKey -> FileIds containing this n-gram
    index: AHashMap<NgramKey, RoaringBitmap>,
    /// Keys modified since last `take_dirty_entries()` call
    dirty: AHashSet<NgramKey>,
    /// Reverse index: FileId -> n-gram keys in that file.
    /// Enables O(ngrams_per_file) removal instead of O(total_ngrams).
    reverse: AHashMap<FileId, Vec<NgramKey>>,
}

impl Default for TrigramIndex {
    fn default() -> Self {
        Self {
            index: AHashMap::new(),
            dirty: AHashSet::new(),
            reverse: AHashMap::new(),
        }
    }
}

/// Upserts and deletes for incremental persistence.
pub type DirtyEntries = (Vec<(Vec<u8>, Vec<u8>)>, Vec<Vec<u8>>);

impl TrigramIndex {
    /// Creates an empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file's content to the index using sparse n-gram extraction.
    pub fn add_file(&mut self, file_id: FileId, content: &str) {
        let keys = ngram::build_all(content.as_bytes());
        let mut seen = AHashSet::with_capacity(keys.len());

        for hash in keys {
            let key = NgramKey::new(hash);
            self.index.entry(key).or_default().insert(file_id.as_u32());
            self.dirty.insert(key);
            seen.insert(key);
        }
        self.reverse.insert(file_id, seen.into_iter().collect());
    }

    /// Removes a file from the index.
    ///
    /// O(ngrams_per_file) via reverse index lookup instead of O(total_ngrams).
    pub fn remove_file(&mut self, file_id: FileId) {
        let id = file_id.as_u32();
        if let Some(keys) = self.reverse.remove(&file_id) {
            for key in keys {
                if let Some(bitmap) = self.index.get_mut(&key) {
                    if bitmap.remove(id) {
                        self.dirty.insert(key);
                    }
                }
            }
        }
    }

    /// Searches for files containing the query substring.
    ///
    /// Uses `build_covering` for minimal n-gram set at query time.
    /// Returns `None` if the query is too short for n-gram filtering.
    pub fn search(&self, query: &str) -> Option<RoaringBitmap> {
        let keys: Vec<NgramKey> = ngram::build_covering(query.as_bytes())
            .into_iter()
            .map(NgramKey::new)
            .collect();

        if keys.is_empty() {
            return None; // Query too short for n-gram filtering
        }

        // Collect bitmaps and check for missing keys
        let mut bitmaps: Vec<&RoaringBitmap> = Vec::with_capacity(keys.len());
        for key in &keys {
            if let Some(bitmap) = self.index.get(key) {
                bitmaps.push(bitmap);
            } else {
                return Some(RoaringBitmap::new()); // No files have this n-gram
            }
        }

        // Start with smallest bitmap (cheapest clone, fastest AND chain)
        bitmaps.sort_by_key(|b| b.len());
        let mut result = bitmaps[0].clone();

        for bitmap in &bitmaps[1..] {
            result &= *bitmap;
            if result.is_empty() {
                break; // Early exit: no files match all n-grams
            }
        }

        Some(result)
    }

    /// Returns the number of unique n-gram keys indexed.
    #[must_use]
    pub fn trigram_count(&self) -> usize {
        self.index.len()
    }

    /// Returns total file references across all n-grams.
    #[must_use]
    pub fn total_refs(&self) -> u64 {
        self.index.values().map(|b| b.len()).sum()
    }

    /// Clears the index, dirty set, and reverse index.
    pub fn clear(&mut self) {
        self.index.clear();
        self.dirty.clear();
        self.reverse.clear();
    }

    /// Returns the number of dirty (modified) keys since last persistence.
    #[must_use]
    pub fn dirty_count(&self) -> usize {
        self.dirty.len()
    }

    /// Takes dirty entries for incremental persistence.
    ///
    /// Returns `(upserts, deletes)`:
    /// - `upserts`: keys with non-empty bitmaps that need INSERT OR REPLACE
    /// - `deletes`: keys whose bitmaps are now empty (need DELETE)
    ///
    /// Clears the dirty set after taking entries.
    pub fn take_dirty_entries(&mut self) -> DirtyEntries {
        let mut upserts = Vec::with_capacity(self.dirty.len());
        let mut deletes = Vec::new();

        for key in self.dirty.drain() {
            match self.index.get(&key) {
                Some(bitmap) if !bitmap.is_empty() => {
                    upserts.push((key.to_le_bytes().to_vec(), Self::bitmap_to_bytes(bitmap)));
                }
                _ => {
                    deletes.push(key.to_le_bytes().to_vec());
                    self.index.remove(&key);
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

    /// Gets the bitmap for a specific n-gram key.
    #[must_use]
    pub fn get_trigram(&self, key: &NgramKey) -> Option<&RoaringBitmap> {
        self.index.get(key)
    }

    /// Sets a key's bitmap directly (for loading from DB).
    pub fn set_trigram(&mut self, key: NgramKey, bitmap: RoaringBitmap) {
        self.index.insert(key, bitmap);
    }

    /// Serializes the entire index to database-compatible entries.
    #[must_use]
    pub fn to_db_entries(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.index
            .iter()
            .map(|(key, bitmap)| (key.to_le_bytes().to_vec(), Self::bitmap_to_bytes(bitmap)))
            .collect()
    }

    /// Loads the index from database entries.
    ///
    /// Takes an iterator of (key_bytes, serialized_bitmap) tuples.
    /// Invalid entries are silently skipped.
    /// The dirty set starts empty since loaded entries are already persisted.
    pub fn from_db_entries<I>(entries: I) -> Self
    where
        I: IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
    {
        let mut index = AHashMap::new();
        let mut reverse: AHashMap<FileId, Vec<NgramKey>> = AHashMap::new();

        for (key_bytes, bitmap_bytes) in entries {
            // NgramKey must be exactly 8 bytes (u64 LE)
            if key_bytes.len() != 8 {
                continue;
            }

            let key = NgramKey::from_le_bytes([
                key_bytes[0],
                key_bytes[1],
                key_bytes[2],
                key_bytes[3],
                key_bytes[4],
                key_bytes[5],
                key_bytes[6],
                key_bytes[7],
            ]);

            if let Some(bitmap) = Self::bitmap_from_bytes(&bitmap_bytes) {
                for file_id_u32 in bitmap.iter() {
                    reverse
                        .entry(FileId::new(file_id_u32))
                        .or_default()
                        .push(key);
                }
                index.insert(key, bitmap);
            }
        }

        Self {
            index,
            dirty: AHashSet::new(),
            reverse,
        }
    }
}

impl std::fmt::Debug for TrigramIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrigramIndex")
            .field("ngram_count", &self.trigram_count())
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
        assert!(results.contains(3));

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

        // "xyz" has no matching n-grams — returns Some(empty bitmap)
        // because n-gram keys exist but don't match any indexed content
        let results = index.search("xyz");
        match results {
            None => {} // Too short for n-gram extraction
            Some(bitmap) => assert!(bitmap.is_empty(), "Should not match any files"),
        }
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
        assert_eq!(index.dirty_count(), 0);
    }

    #[test]
    fn test_dirty_tracking_remove_produces_deletes() {
        let mut index = TrigramIndex::new();
        index.add_file(FileId::new(1), "xyzxyz unique content here");
        let _ = index.take_dirty_entries();
        assert_eq!(index.dirty_count(), 0);

        index.remove_file(FileId::new(1));
        assert!(index.dirty_count() > 0);

        let (upserts, deletes) = index.take_dirty_entries();
        assert!(!deletes.is_empty());
        assert!(upserts.is_empty());
    }

    #[test]
    fn test_dirty_tracking_incremental() {
        let mut index = TrigramIndex::new();
        index.add_file(FileId::new(1), "authentication");
        let _ = index.take_dirty_entries();
        assert_eq!(index.dirty_count(), 0);

        index.add_file(FileId::new(2), "authorization");
        let dirty_count = index.dirty_count();
        let total_count = index.trigram_count();
        assert!(dirty_count <= total_count);
        assert!(dirty_count > 0);
    }

    #[test]
    fn test_from_db_entries_starts_clean() {
        let mut index = TrigramIndex::new();
        index.add_file(FileId::new(1), "hello world");
        let entries = index.to_db_entries();

        let loaded = TrigramIndex::from_db_entries(entries);
        assert_eq!(loaded.dirty_count(), 0);
        assert!(loaded.trigram_count() > 0);
    }

    #[test]
    fn test_db_roundtrip() {
        let mut index = TrigramIndex::new();
        index.add_file(FileId::new(1), "authentication system");
        index.add_file(FileId::new(2), "authorization module");

        let entries = index.to_db_entries();
        let loaded = TrigramIndex::from_db_entries(entries);

        // Search should work the same after roundtrip
        let results = loaded.search("auth").unwrap();
        assert!(results.contains(1));
        assert!(results.contains(2));
    }

    /// 6a: from_db_entries silently rejects old 3-byte keys and garbage.
    #[test]
    fn test_from_db_entries_rejects_wrong_length_keys() {
        let mut index = TrigramIndex::new();
        index.add_file(FileId::new(1), "authentication");
        let valid_entries = index.to_db_entries();
        assert!(!valid_entries.is_empty());

        // Mix valid 8-byte entries with old 3-byte and garbage entries
        let bitmap_bytes = TrigramIndex::bitmap_to_bytes(&{
            let mut b = RoaringBitmap::new();
            b.insert(99);
            b
        });
        let mut mixed = valid_entries.clone();
        mixed.push((b"aut".to_vec(), bitmap_bytes.clone())); // old 3-byte key
        mixed.push((b"x".to_vec(), bitmap_bytes.clone())); // 1-byte garbage
        mixed.push((vec![0u8; 16], bitmap_bytes)); // 16-byte garbage

        let loaded = TrigramIndex::from_db_entries(mixed);
        // Only valid 8-byte entries should survive
        assert_eq!(loaded.trigram_count(), index.trigram_count());
        // File 99 from the garbage entries should NOT be searchable
        let results = loaded.search("authentication").unwrap();
        assert!(results.contains(1));
        assert!(!results.contains(99));
    }

    /// 6c: Full persistence roundtrip through actual SQLite.
    #[test]
    fn test_full_sqlite_persistence_roundtrip() {
        use crate::db::Database;

        let db = Database::in_memory().unwrap();
        let mut index = TrigramIndex::new();
        index.add_file(FileId::new(1), "authentication system");
        index.add_file(FileId::new(2), "authorization module");

        // Persist via dirty entries (production path)
        let (upserts, deletes) = index.take_dirty_entries();
        db.save_dirty_trigrams(&upserts, &deletes).unwrap();

        // Load from DB
        let db_entries = db.load_all_trigrams().unwrap();
        let loaded = TrigramIndex::from_db_entries(db_entries);

        // Search should work after real SQLite roundtrip
        let results = loaded.search("auth").unwrap();
        assert!(results.contains(1));
        assert!(results.contains(2));

        // Verify counts match
        assert_eq!(loaded.trigram_count(), index.trigram_count());
    }
}
