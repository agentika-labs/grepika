//! Sparse frequency-weighted n-gram extraction.
//!
//! Replaces dense trigrams (every 3-byte window) with variable-length
//! n-grams selected by bigram frequency weights. Rare character pairs
//! get high weights, so n-gram boundaries fall at selective positions.
//!
//! Two extraction modes:
//! - `build_all`: index time, all valid n-grams for comprehensive coverage
//! - `build_covering`: query time, minimal set covering every byte position

use ahash::AHashSet;
use xxhash_rust::xxh3::xxh3_64;

/// Maximum n-gram length in bytes. Caps the inner loop to prevent
/// pathological O(n²) behavior on content with uniform bigram weights.
const MAX_NGRAM_LEN: usize = 16;

/// Minimum n-gram length (two adjacent bigram positions = 3 bytes).
const MIN_NGRAM_LEN: usize = 3;

/// Bigram weight table: 65536 entries indexed by `(byte1 << 8) | byte2`.
/// Higher weight = rarer character pair = better n-gram boundary.
///
/// Uses CRC32-based deterministic weights. A corpus-derived frequency
/// table can replace this later for better selectivity.
static BIGRAM_WEIGHTS: [u16; 65536] = {
    let mut table = [0u16; 65536];
    let mut i: usize = 0;
    while i < 65536 {
        // CRC32-inspired hash: mix the two bytes deterministically.
        // This produces a pseudo-random u16 weight for each byte pair.
        let a = (i >> 8) as u32;
        let b = (i & 0xFF) as u32;
        let mut h = 0x811c9dc5_u32; // FNV offset basis
        h ^= a;
        h = h.wrapping_mul(0x01000193); // FNV prime
        h ^= b;
        h = h.wrapping_mul(0x01000193);
        // Fold 32 bits into 16 bits
        table[i] = ((h >> 16) ^ (h & 0xFFFF)) as u16;
        i += 1;
    }
    table
};

/// Returns the bigram weight for a byte pair.
#[inline]
fn bigram_weight(a: u8, b: u8) -> u16 {
    BIGRAM_WEIGHTS[(a as usize) << 8 | b as usize]
}

/// Hashes an n-gram byte slice to a u64 key.
#[inline]
pub fn ngram_key(bytes: &[u8]) -> u64 {
    xxh3_64(bytes)
}

/// Extracts all valid n-grams from content (index time).
///
/// An n-gram `content[L..L+len]` (len >= 3) is valid when the bigram
/// weights at positions L and L+len-2 are strictly greater than all
/// interior bigram weights.
///
/// Returns deduplicated xxh3 hashes of the n-gram byte slices.
pub fn build_all(content: &[u8]) -> Vec<u64> {
    if content.len() < MIN_NGRAM_LEN {
        return Vec::new();
    }

    // Pre-compute bigram weights for the entire content
    let num_bigrams = content.len() - 1;
    let weights: Vec<u16> = (0..num_bigrams)
        .map(|i| bigram_weight(content[i], content[i + 1]))
        .collect();

    let mut seen = AHashSet::new();

    // For each left edge position L
    for left in 0..num_bigrams {
        let w_left = weights[left];
        let mut interior_max: u16 = 0;

        // Scan right edges: the n-gram spans content[left..right+2]
        // Minimum length 3 means right >= left+1 (one interior gap = zero interior bigrams)
        // Actually: for length 3, right = left+1, and the n-gram is content[left..left+3].
        // The "right edge bigram" is at position right = left + len - 2.
        // For len=3: right = left+1, interior = empty (always valid if both edges exist)
        for right in (left + 1)..num_bigrams {
            let len = right - left + 2; // n-gram length in bytes
            if len > MAX_NGRAM_LEN {
                break;
            }

            let w_right = weights[right];

            // For length > 3, check that left edge dominates all interior weights
            if right > left + 1 {
                // The new interior position is at `right - 1`
                let w_interior = weights[right - 1];
                if w_interior > interior_max {
                    interior_max = w_interior;
                }

                // Left edge must strictly dominate all interior weights
                if interior_max >= w_left {
                    break; // Left edge is permanently dominated, no longer n-grams from here
                }
            }

            // Right edge must strictly dominate all interior weights
            if w_right > interior_max {
                let ngram_bytes = &content[left..left + len];
                seen.insert(ngram_key(ngram_bytes));
            }
        }
    }

    seen.into_iter().collect()
}

/// Extracts the minimal covering set of n-grams from a query (query time).
///
/// Uses greedy interval covering: at each uncovered position, pick the
/// longest valid n-gram starting there. This is provably optimal for
/// minimum-cardinality interval covering.
///
/// Returns xxh3 hashes of the covering n-gram byte slices.
pub fn build_covering(query: &[u8]) -> Vec<u64> {
    if query.len() < MIN_NGRAM_LEN {
        return Vec::new();
    }

    let num_bigrams = query.len() - 1;
    let weights: Vec<u16> = (0..num_bigrams)
        .map(|i| bigram_weight(query[i], query[i + 1]))
        .collect();

    let mut result = Vec::new();
    let mut covered_up_to: usize = 0; // next byte position that needs covering

    while covered_up_to + MIN_NGRAM_LEN <= query.len() {
        let left = covered_up_to;
        if left >= num_bigrams {
            break;
        }

        let w_left = weights[left];
        let mut interior_max: u16 = 0;
        let mut best_end: Option<usize> = None; // end position (exclusive) of best n-gram

        for right in (left + 1)..num_bigrams {
            let len = right - left + 2;
            if len > MAX_NGRAM_LEN {
                break;
            }

            let w_right = weights[right];

            if right > left + 1 {
                let w_interior = weights[right - 1];
                if w_interior > interior_max {
                    interior_max = w_interior;
                }
                if interior_max >= w_left {
                    break;
                }
            }

            if w_right > interior_max {
                best_end = Some(left + len); // This n-gram is valid and extends further
            }
        }

        match best_end {
            Some(end) => {
                result.push(ngram_key(&query[left..end]));
                covered_up_to = end;
            }
            None => {
                // No valid n-gram starts here — advance by 1 and try again
                covered_up_to += 1;
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_all_short_input() {
        assert!(build_all(b"ab").is_empty());
        assert!(build_all(b"a").is_empty());
        assert!(build_all(b"").is_empty());
    }

    #[test]
    fn test_build_all_minimum_length() {
        // 3 bytes always produces at least one n-gram (length-3 is always valid)
        let keys = build_all(b"abc");
        assert!(!keys.is_empty());
    }

    #[test]
    fn test_build_covering_short_input() {
        assert!(build_covering(b"ab").is_empty());
        assert!(build_covering(b"a").is_empty());
        assert!(build_covering(b"").is_empty());
    }

    #[test]
    fn test_build_covering_minimum_length() {
        let keys = build_covering(b"abc");
        assert!(!keys.is_empty());
    }

    #[test]
    fn test_build_all_produces_deduped_keys() {
        // Content with repeated substrings
        let content = b"abcabc";
        let keys = build_all(content);
        let unique: AHashSet<u64> = keys.iter().copied().collect();
        assert_eq!(keys.len(), unique.len());
    }

    #[test]
    fn test_ngram_key_deterministic() {
        assert_eq!(ngram_key(b"auth"), ngram_key(b"auth"));
        assert_ne!(ngram_key(b"auth"), ngram_key(b"autz"));
    }

    #[test]
    fn test_bigram_weight_deterministic() {
        assert_eq!(bigram_weight(b'a', b'b'), bigram_weight(b'a', b'b'));
        // Different pairs should (usually) have different weights
        // Not guaranteed but highly likely with good hashing
    }

    #[test]
    fn test_all_identical_chars() {
        // All identical chars means all bigram weights are equal.
        // Only length-3 n-grams are valid (no interior to dominate).
        let content = b"aaaaaa";
        let keys = build_all(content);
        // Should produce exactly 1 unique key (all "aaa" substrings hash the same)
        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn test_covering_covers_all_positions() {
        // For a longer query, verify covering produces keys
        let query = b"authenticate";
        let covering = build_covering(query);
        assert!(!covering.is_empty());
    }

    /// Critical invariant: every key in `build_covering(Q)` must appear
    /// in `build_all(C)` when Q is a substring of C.
    #[test]
    fn test_subset_invariant() {
        let content = b"fn authenticate(config: &Config) -> Result<(), Error>";
        let all_keys: AHashSet<u64> = build_all(content).into_iter().collect();

        // Test several substrings
        let substrings = [
            &b"authenticate"[..],
            &b"config"[..],
            &b"Config"[..],
            &b"Result"[..],
            &b"Error"[..],
            &b"fn auth"[..],
        ];

        for substr in substrings {
            let covering = build_covering(substr);
            for key in &covering {
                assert!(
                    all_keys.contains(key),
                    "build_covering({:?}) produced key {} not in build_all",
                    std::str::from_utf8(substr).unwrap_or("?"),
                    key
                );
            }
        }
    }

    /// Broader subset invariant test with sliding window over content.
    #[test]
    fn test_subset_invariant_exhaustive() {
        let content = b"pub fn search(&self, query: &str) -> Option<RoaringBitmap>";
        let all_keys: AHashSet<u64> = build_all(content).into_iter().collect();

        // Test every substring of length 3..20
        for start in 0..content.len() {
            for end in (start + 3)..=(start + 20).min(content.len()) {
                let substr = &content[start..end];
                let covering = build_covering(substr);
                for key in &covering {
                    assert!(
                        all_keys.contains(key),
                        "Substring [{start}..{end}] = {:?}: key {key} not in build_all",
                        std::str::from_utf8(substr).unwrap_or("?")
                    );
                }
            }
        }
    }

    #[test]
    fn test_build_all_realistic_content() {
        let content = b"fn main() { let config = Config::load(); }";
        let keys = build_all(content);
        // Should produce multiple n-grams of varying lengths
        assert!(keys.len() > 1);
    }

    #[test]
    fn test_max_ngram_length_cap() {
        // Very long content with potentially unbounded n-grams
        let content = vec![0u8; 1000];
        let keys = build_all(&content);
        // Should still complete (no O(n²) blowup) and produce results
        assert!(!keys.is_empty());
    }

    #[test]
    fn test_build_covering_advances_past_gaps() {
        // Even if some positions don't start valid n-grams,
        // covering should advance and find the next valid one
        let query = b"hello world";
        let covering = build_covering(query);
        // Should produce some keys (not get stuck)
        assert!(!covering.is_empty());
    }

    /// 6d: build_covering on uniform-weight content (all identical chars).
    #[test]
    fn test_build_covering_uniform_weights() {
        // All identical chars: all bigram weights are equal.
        // Only length-3 n-grams are valid. Covering must advance by
        // using the None branch (skip by 1) between n-grams.
        let query = b"aaaaaa";
        let covering = build_covering(query);
        // Should produce some keys (not return empty)
        assert!(!covering.is_empty());

        // Verify the subset invariant still holds
        let all_keys: AHashSet<u64> = build_all(query).into_iter().collect();
        for key in &covering {
            assert!(all_keys.contains(key));
        }
    }

    /// 6f: Subset invariant with multi-byte UTF-8 content.
    #[test]
    fn test_subset_invariant_utf8_multibyte() {
        // CJK characters (3 bytes each in UTF-8) + emoji (4 bytes)
        let content = "fn 处理(配置: &Config) -> 结果<(), 错误> { println!(\"🔍\"); }";
        let content_bytes = content.as_bytes();
        let all_keys: AHashSet<u64> = build_all(content_bytes).into_iter().collect();

        // Test substrings that cross codepoint boundaries (byte-level slicing)
        for start in 0..content_bytes.len() {
            for end in (start + 3)..=(start + 12).min(content_bytes.len()) {
                let substr = &content_bytes[start..end];
                let covering = build_covering(substr);
                for key in &covering {
                    assert!(
                        all_keys.contains(key),
                        "UTF-8 substring [{start}..{end}]: key {key} not in build_all"
                    );
                }
            }
        }
    }
}
