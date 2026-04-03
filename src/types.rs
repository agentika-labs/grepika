//! Type-safe newtypes for grepika.
//!
//! These newtypes provide compile-time safety and semantic clarity
//! for core domain concepts.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Database identifier for indexed files.
///
/// Using u32 supports ~4 billion files which is sufficient for any codebase.
/// The newtype prevents accidental mixing with other integer values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileId(pub u32);

impl FileId {
    #[must_use]
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let id = self.0;
        write!(f, "file:{id}")
    }
}

impl From<u32> for FileId {
    fn from(id: u32) -> Self {
        Self(id)
    }
}

impl From<FileId> for u32 {
    fn from(id: FileId) -> Self {
        id.0
    }
}

/// Relevance score in range [0.0, 1.0].
///
/// Saturating constructor ensures scores never exceed bounds,
/// making score merging operations safe.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Score(f64);

impl Score {
    /// Zero relevance score.
    pub const ZERO: Self = Self(0.0);

    /// Maximum relevance score.
    pub const MAX: Self = Self(1.0);

    /// Creates a new score, saturating to [0.0, 1.0] bounds.
    /// NaN is treated as zero (defensive hardening).
    #[must_use]
    pub fn new(value: f64) -> Self {
        if value.is_nan() {
            return Self(0.0);
        }
        Self(value.clamp(0.0, 1.0))
    }

    /// Creates a score from a value already known to be in bounds.
    ///
    /// # Correctness
    /// Caller must ensure value is in [0.0, 1.0].
    #[must_use]
    pub const fn new_unchecked(value: f64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_f64(self) -> f64 {
        self.0
    }

    /// Combines two scores with bounded addition.
    /// The result is clamped to [0.0, 1.0].
    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        Self::new(self.0 + other.0)
    }

    /// Applies a weight factor to this score.
    #[must_use]
    pub fn weighted(self, weight: f64) -> Self {
        Self::new(self.0 * weight)
    }
}

impl Default for Score {
    fn default() -> Self {
        Self::ZERO
    }
}

impl fmt::Display for Score {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3}", self.0)
    }
}

/// N-gram key for sparse substring indexing.
///
/// Represents the xxh3_64 hash of a variable-length n-gram byte slice.
/// Sparse n-grams are selected by bigram frequency weights, producing
/// more selective posting lists than dense 3-byte trigrams.
///
/// Hash collisions only cause false positives (more files to grep),
/// never false negatives — grep verifies actual content.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct NgramKey(pub u64);

/// Backward compatibility alias.
pub type Trigram = NgramKey;

impl NgramKey {
    #[must_use]
    pub const fn new(hash: u64) -> Self {
        Self(hash)
    }

    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Serializes to little-endian bytes for database storage.
    #[must_use]
    pub const fn to_le_bytes(self) -> [u8; 8] {
        self.0.to_le_bytes()
    }

    /// Deserializes from little-endian bytes.
    #[must_use]
    pub const fn from_le_bytes(bytes: [u8; 8]) -> Self {
        Self(u64::from_le_bytes(bytes))
    }
}

impl fmt::Debug for NgramKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NgramKey({:016x})", self.0)
    }
}

impl fmt::Display for NgramKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

// Compile-time assertions for thread safety.
// These ensure Send+Sync remain implemented and catch regressions.
#[cfg(test)]
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}

    // Core newtypes
    assert_send_sync::<FileId>();
    assert_send_sync::<Score>();
    assert_send_sync::<NgramKey>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_score_saturation() {
        assert_eq!(Score::new(1.5).as_f64(), 1.0);
        assert_eq!(Score::new(-0.5).as_f64(), 0.0);
        assert_eq!(Score::new(0.5).as_f64(), 0.5);
    }

    #[test]
    fn test_score_merge() {
        let s1 = Score::new(0.6);
        let s2 = Score::new(0.7);
        assert_eq!(s1.merge(s2).as_f64(), 1.0); // Saturated
    }

    #[test]
    fn test_ngram_key_roundtrip() {
        let key = NgramKey::new(0x1234567890abcdef);
        let bytes = key.to_le_bytes();
        let restored = NgramKey::from_le_bytes(bytes);
        assert_eq!(key, restored);
    }

    #[test]
    fn test_ngram_key_display() {
        let key = NgramKey::new(0xff);
        assert_eq!(format!("{key}"), "00000000000000ff");
    }

    #[test]
    fn test_score_nan_becomes_zero() {
        let score = Score::new(f64::NAN);
        assert_eq!(score.as_f64(), 0.0);
    }

    #[test]
    fn test_score_infinity_saturates() {
        assert_eq!(Score::new(f64::INFINITY).as_f64(), 1.0);
    }

    #[test]
    fn test_score_neg_infinity_saturates() {
        assert_eq!(Score::new(f64::NEG_INFINITY).as_f64(), 0.0);
    }

    #[test]
    fn test_file_id_roundtrip() {
        let id = FileId::new(42);
        assert_eq!(id.as_u32(), 42);
        assert_eq!(u32::from(id), 42);
    }
}
