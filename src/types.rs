//! Type-safe newtypes for agentika-grep.
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
    #[must_use]
    pub fn new(value: f64) -> Self {
        Self(value.clamp(0.0, 1.0))
    }

    /// Creates a score from a value already known to be in bounds.
    ///
    /// # Safety
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

/// Three-byte trigram for substring indexing.
///
/// Trigrams enable fast substring search by decomposing strings
/// into overlapping 3-character sequences. For example:
/// "auth" → ["aut", "uth"]
///
/// Finding files containing "auth" means finding files that
/// contain ALL of its trigrams.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Trigram(pub [u8; 3]);

impl Trigram {
    #[must_use]
    pub const fn new(bytes: [u8; 3]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 3] {
        &self.0
    }

    /// Extracts all trigrams from a string.
    ///
    /// Returns an iterator over trigrams. Short strings (< 3 bytes)
    /// yield no trigrams.
    pub fn extract(s: &str) -> impl Iterator<Item = Trigram> + '_ {
        let bytes = s.as_bytes();
        bytes.windows(3).map(|w| Trigram([w[0], w[1], w[2]]))
    }

    /// Extracts trigrams from bytes.
    pub fn from_bytes(bytes: &[u8]) -> impl Iterator<Item = Trigram> + '_ {
        bytes.windows(3).map(|w| Trigram([w[0], w[1], w[2]]))
    }
}

impl fmt::Debug for Trigram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Try to display as UTF-8 string if valid
        if let Ok(s) = std::str::from_utf8(&self.0) {
            write!(f, "Trigram({s:?})")
        } else {
            let [a, b, c] = self.0;
            write!(f, "Trigram({a:02x}{b:02x}{c:02x})")
        }
    }
}

impl fmt::Display for Trigram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(s) = std::str::from_utf8(&self.0) {
            write!(f, "{s}")
        } else {
            let [a, b, c] = self.0;
            write!(f, "{a:02x}{b:02x}{c:02x}")
        }
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
    assert_send_sync::<Trigram>();
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
    fn test_trigram_extraction() {
        let trigrams: Vec<_> = Trigram::extract("auth").collect();
        assert_eq!(trigrams.len(), 2);
        assert_eq!(trigrams[0].0, *b"aut");
        assert_eq!(trigrams[1].0, *b"uth");
    }

    #[test]
    fn test_trigram_short_string() {
        let trigrams: Vec<_> = Trigram::extract("ab").collect();
        assert!(trigrams.is_empty());
    }

    #[test]
    fn test_file_id_roundtrip() {
        let id = FileId::new(42);
        assert_eq!(id.as_u32(), 42);
        assert_eq!(u32::from(id), 42);
    }
}
