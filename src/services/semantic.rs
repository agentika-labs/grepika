//! Semantic embedding support (optional `semantic` feature).
//!
//! The vector math and BLOB codec are always compiled (cheap, testable). The
//! actual embedder wraps `fastembed` and is gated behind the `semantic`
//! feature so the default binary stays lean and dependency-free. When the
//! feature is off, [`Embedder::load`] returns `None` and indexing/search skip
//! the semantic backend entirely.

/// Cosine similarity of two equal-length vectors. Returns 0.0 for a length
/// mismatch or a zero-magnitude vector.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Encodes a vector as little-endian f32 bytes for BLOB storage.
pub fn encode(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Decodes a little-endian f32 BLOB back into a vector. Trailing bytes that
/// don't form a full f32 are ignored.
pub fn decode(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// A loaded embedding model. Only functional under the `semantic` feature.
pub struct Embedder {
    #[cfg(feature = "semantic")]
    model: fastembed::TextEmbedding,
}

impl Embedder {
    /// Loads the embedding model. Returns `None` when the `semantic` feature
    /// is disabled, or `Some(Err(..))` if model initialization fails.
    #[cfg(feature = "semantic")]
    pub fn load() -> Option<Result<Self, String>> {
        use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
        let res = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15).with_show_download_progress(false),
        )
        .map(|model| Self { model })
        .map_err(|e| e.to_string());
        Some(res)
    }

    #[cfg(not(feature = "semantic"))]
    pub fn load() -> Option<Result<Self, String>> {
        None
    }

    /// Embeds a batch of texts. Errors out (or returns empty) when the feature
    /// is disabled.
    #[cfg(feature = "semantic")]
    pub fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
        self.model.embed(texts, None).map_err(|e| e.to_string())
    }

    #[cfg(not(feature = "semantic"))]
    pub fn embed(&self, _texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
        Ok(Vec::new())
    }
}

/// Loads the embedding model once, wrapped in a `Mutex` so holders stay
/// `Sync` despite the model's non-`Sync` session. Returns `None` when the
/// `semantic` feature is disabled or model load fails (logged).
pub fn load_shared() -> Option<std::sync::Mutex<Embedder>> {
    match Embedder::load() {
        Some(Ok(e)) => Some(std::sync::Mutex::new(e)),
        Some(Err(e)) => {
            tracing::warn!("semantic embedder failed to load, semantic disabled: {e}");
            None
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0); // length mismatch
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0); // zero vector
    }

    #[test]
    fn codec_roundtrip() {
        let v = vec![0.5f32, -1.25, 3.0, 0.0];
        assert_eq!(decode(&encode(&v)), v);
    }
}
