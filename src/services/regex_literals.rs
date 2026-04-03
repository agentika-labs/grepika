//! Regex AST literal extraction for trigram pre-filtering.
//!
//! Parses regex patterns with `regex-syntax` to extract literal byte sequences,
//! enabling the trigram index to pre-filter files even for regex queries.

use regex_syntax::hir::{Hir, HirKind};
use regex_syntax::Parser;

/// Extracts literal string segments from a regex pattern for trigram pre-filtering.
///
/// Parses the regex AST and collects contiguous literal byte sequences.
/// Only returns segments >= 3 bytes (the minimum for trigram matching).
/// Returns an empty Vec if parsing fails or no usable literals are found.
#[must_use]
pub fn extract_literals(pattern: &str) -> Vec<String> {
    let hir = match Parser::new().parse(pattern) {
        Ok(hir) => hir,
        Err(_) => return Vec::new(),
    };

    let mut segments = Vec::new();
    let mut buf = Vec::new();
    extract_from_hir(&hir, &mut buf, &mut segments);
    flush(&mut buf, &mut segments);

    segments
}

/// Flushes accumulated literal bytes into a segment if >= 3 bytes and valid UTF-8.
fn flush(buf: &mut Vec<u8>, segments: &mut Vec<String>) {
    if buf.len() >= 3 {
        if let Ok(s) = std::str::from_utf8(buf) {
            segments.push(s.to_owned());
        }
    }
    buf.clear();
}

/// Recursively walks the HIR tree, accumulating literal bytes and flushing
/// segments at non-literal boundaries.
fn extract_from_hir(hir: &Hir, buf: &mut Vec<u8>, segments: &mut Vec<String>) {
    match hir.kind() {
        HirKind::Literal(lit) => {
            buf.extend_from_slice(&lit.0);
        }
        HirKind::Concat(subs) => {
            for sub in subs {
                extract_from_hir(sub, buf, segments);
            }
        }
        HirKind::Alternation(_) => {
            // Don't extract from alternation branches — the caller AND-intersects
            // literals, but alternation has OR semantics. Extracting would cause
            // false negatives in the trigram pre-filter.
            flush(buf, segments);
        }
        HirKind::Repetition(rep) => {
            flush(buf, segments);
            extract_from_hir(&rep.sub, buf, segments);
            flush(buf, segments);
        }
        HirKind::Capture(cap) => {
            // Recurse into the capture group without breaking the literal sequence
            extract_from_hir(&cap.sub, buf, segments);
        }
        HirKind::Class(_) | HirKind::Look(_) => {
            flush(buf, segments);
        }
        HirKind::Empty => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_literal() {
        let literals = extract_literals("authenticate");
        assert_eq!(literals, vec!["authenticate"]);
    }

    #[test]
    fn test_regex_with_literal_prefix() {
        // Escaped parens are literals in regex, so the whole string is one segment
        let literals = extract_literals(r"authenticate\(\)");
        assert!(literals.contains(&"authenticate()".to_string()));
    }

    #[test]
    fn test_regex_with_short_literal() {
        // "fn" is only 2 bytes, should be filtered out
        let literals = extract_literals(r"fn\s+\w+");
        assert!(literals.iter().all(|l| l.len() >= 3));
    }

    #[test]
    fn test_multiple_literals() {
        let literals = extract_literals(r"impl\s+Display\s+for");
        assert!(literals.contains(&"impl".to_string()));
        assert!(literals.contains(&"Display".to_string()));
        assert!(literals.contains(&"for".to_string()));
    }

    #[test]
    fn test_class_breaks_literal() {
        let literals = extract_literals(r"[A-Z]Config");
        assert!(literals.contains(&"Config".to_string()));
    }

    #[test]
    fn test_invalid_regex_returns_empty() {
        let literals = extract_literals(r"(unclosed");
        assert!(literals.is_empty());
    }

    #[test]
    fn test_empty_pattern() {
        let literals = extract_literals("");
        assert!(literals.is_empty());
    }

    #[test]
    fn test_no_literals() {
        // Pattern with no literal segments >= 3 bytes
        let literals = extract_literals(r"\d+\s+\w+");
        assert!(literals.is_empty());
    }

    #[test]
    fn test_alternation_not_extracted() {
        // Alternation branches are not extracted because the caller AND-intersects
        // literals, which would incorrectly filter out valid matches
        let literals = extract_literals(r"(authenticate|authorize)");
        assert!(literals.is_empty());
    }

    #[test]
    fn test_alternation_with_surrounding_literals() {
        // Literals outside alternation should still be extracted
        let literals = extract_literals(r"prefix_(foo|bar)_suffix");
        assert!(literals.contains(&"prefix_".to_string()));
        assert!(literals.contains(&"_suffix".to_string()));
        // But "foo" and "bar" from inside the alternation should NOT appear
        assert!(!literals.contains(&"foo".to_string()));
        assert!(!literals.contains(&"bar".to_string()));
    }

    #[test]
    fn test_dot_star_breaks_literal() {
        let literals = extract_literals(r"foo.*bar");
        // Both "foo" and "bar" are exactly 3 bytes, should be included
        assert!(literals.contains(&"foo".to_string()));
        assert!(literals.contains(&"bar".to_string()));
    }
}
