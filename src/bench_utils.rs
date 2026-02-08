//! Token efficiency measurement utilities for benchmarks.
//!
//! Provides metrics for comparing agentika-grep output against
//! ripgrep/Claude Grep output to quantify token savings.

use serde::Serialize;

/// Token metrics for benchmark comparison.
#[derive(Debug, Clone, Default)]
pub struct TokenMetrics {
    /// Raw output size in bytes
    pub output_bytes: usize,
    /// Estimated token count (bytes / 4 approximation for Claude)
    pub estimated_tokens: usize,
    /// Number of result items returned
    pub result_count: usize,
    /// Number of unique files found
    pub files_found: usize,
}

impl TokenMetrics {
    /// Creates metrics from a serializable output.
    pub fn from_output<T: Serialize>(output: &T, result_count: usize, files_found: usize) -> Self {
        let json = serde_json::to_string(output).unwrap_or_default();
        let output_bytes = json.len();
        Self {
            output_bytes,
            estimated_tokens: output_bytes / 4, // Claude tokenization approximation
            result_count,
            files_found,
        }
    }

    /// Creates metrics from raw JSON string.
    pub fn from_json(json: &str, result_count: usize, files_found: usize) -> Self {
        let output_bytes = json.len();
        Self {
            output_bytes,
            estimated_tokens: output_bytes / 4,
            result_count,
            files_found,
        }
    }

    /// Creates metrics from ripgrep-style output.
    ///
    /// Simulates Claude's Grep tool output format with context lines.
    pub fn from_ripgrep_output(output: &str, result_count: usize, files_found: usize) -> Self {
        let output_bytes = output.len();
        Self {
            output_bytes,
            estimated_tokens: output_bytes / 4,
            result_count,
            files_found,
        }
    }

    /// Files discovered per 1K estimated tokens.
    pub fn result_density(&self) -> f64 {
        if self.estimated_tokens == 0 {
            return 0.0;
        }
        (self.files_found as f64 / self.estimated_tokens as f64) * 1000.0
    }

    /// Results per 1K bytes.
    pub fn results_per_kb(&self) -> f64 {
        if self.output_bytes == 0 {
            return 0.0;
        }
        (self.result_count as f64 / self.output_bytes as f64) * 1024.0
    }

    /// Calculate percentage savings compared to another metric.
    pub fn savings_vs(&self, other: &TokenMetrics) -> f64 {
        if other.output_bytes == 0 {
            return 0.0;
        }
        let savings = other.output_bytes as f64 - self.output_bytes as f64;
        (savings / other.output_bytes as f64) * 100.0
    }
}

/// Comparison result between two approaches.
#[derive(Debug, Clone)]
pub struct ComparisonResult {
    pub query: String,
    pub agentika: TokenMetrics,
    pub ripgrep: TokenMetrics,
    pub savings_percent: f64,
}

impl ComparisonResult {
    pub fn new(query: impl Into<String>, agentika: TokenMetrics, ripgrep: TokenMetrics) -> Self {
        let savings_percent = agentika.savings_vs(&ripgrep);
        Self {
            query: query.into(),
            agentika,
            ripgrep,
            savings_percent,
        }
    }
}

/// Break-even analysis for MCP schema overhead.
#[derive(Debug, Clone)]
pub struct BreakEvenAnalysis {
    /// One-time MCP schema overhead in bytes
    pub schema_bytes: usize,
    /// Estimated schema tokens
    pub schema_tokens: usize,
    /// Average per-query savings in bytes
    pub avg_savings_bytes: f64,
    /// Average per-query savings in tokens
    pub avg_savings_tokens: f64,
    /// Number of queries needed to break even
    pub break_even_queries: usize,
}

impl BreakEvenAnalysis {
    pub fn calculate(schema_bytes: usize, comparisons: &[ComparisonResult]) -> Self {
        let schema_tokens = schema_bytes / 4;

        if comparisons.is_empty() {
            return Self {
                schema_bytes,
                schema_tokens,
                avg_savings_bytes: 0.0,
                avg_savings_tokens: 0.0,
                break_even_queries: usize::MAX,
            };
        }

        // Calculate average savings per query
        let total_savings: f64 = comparisons
            .iter()
            .map(|c| c.ripgrep.output_bytes as f64 - c.agentika.output_bytes as f64)
            .sum();

        let avg_savings_bytes = total_savings / comparisons.len() as f64;
        let avg_savings_tokens = avg_savings_bytes / 4.0;

        // Break-even: schema_tokens / avg_savings_tokens
        let break_even_queries = if avg_savings_tokens > 0.0 {
            (schema_tokens as f64 / avg_savings_tokens).ceil() as usize
        } else {
            usize::MAX
        };

        Self {
            schema_bytes,
            schema_tokens,
            avg_savings_bytes,
            avg_savings_tokens,
            break_even_queries,
        }
    }
}

/// Statistical summary for benchmark results.
#[derive(Debug, Clone)]
pub struct BenchmarkStats {
    pub mean: f64,
    pub median: f64,
    pub std_dev: f64,
    pub cv_percent: f64, // Coefficient of variation
    pub min: f64,
    pub max: f64,
    pub sample_count: usize,
}

impl BenchmarkStats {
    pub fn from_samples(samples: &[f64]) -> Self {
        if samples.is_empty() {
            return Self {
                mean: 0.0,
                median: 0.0,
                std_dev: 0.0,
                cv_percent: 0.0,
                min: 0.0,
                max: 0.0,
                sample_count: 0,
            };
        }

        let n = samples.len() as f64;
        let mean = samples.iter().sum::<f64>() / n;

        let variance = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
        let std_dev = variance.sqrt();

        let cv_percent = if mean > 0.0 {
            (std_dev / mean) * 100.0
        } else {
            0.0
        };

        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let median = if sorted.len().is_multiple_of(2) {
            (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2]) / 2.0
        } else {
            sorted[sorted.len() / 2]
        };

        let min = *sorted.first().unwrap_or(&0.0);
        let max = *sorted.last().unwrap_or(&0.0);

        Self {
            mean,
            median,
            std_dev,
            cv_percent,
            min,
            max,
            sample_count: samples.len(),
        }
    }

    /// Returns true if CV% is below threshold (reliable measurement).
    pub fn is_reliable(&self, threshold: f64) -> bool {
        self.cv_percent < threshold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_metrics_from_json() {
        let json = r#"{"results":[{"path":"src/main.rs","score":0.95}],"total":1}"#;
        let metrics = TokenMetrics::from_json(json, 1, 1);

        assert_eq!(metrics.output_bytes, json.len());
        assert_eq!(metrics.estimated_tokens, json.len() / 4);
        assert_eq!(metrics.result_count, 1);
        assert_eq!(metrics.files_found, 1);
    }

    #[test]
    fn test_savings_calculation() {
        let agentika = TokenMetrics {
            output_bytes: 100,
            estimated_tokens: 25,
            result_count: 5,
            files_found: 5,
        };

        let ripgrep = TokenMetrics {
            output_bytes: 400,
            estimated_tokens: 100,
            result_count: 5,
            files_found: 5,
        };

        let savings = agentika.savings_vs(&ripgrep);
        assert!((savings - 75.0).abs() < 0.01); // 75% savings
    }

    #[test]
    fn test_break_even_analysis() {
        let comparisons = vec![
            ComparisonResult::new(
                "test1",
                TokenMetrics {
                    output_bytes: 100,
                    estimated_tokens: 25,
                    result_count: 5,
                    files_found: 5,
                },
                TokenMetrics {
                    output_bytes: 400,
                    estimated_tokens: 100,
                    result_count: 5,
                    files_found: 5,
                },
            ),
            ComparisonResult::new(
                "test2",
                TokenMetrics {
                    output_bytes: 200,
                    estimated_tokens: 50,
                    result_count: 10,
                    files_found: 10,
                },
                TokenMetrics {
                    output_bytes: 600,
                    estimated_tokens: 150,
                    result_count: 10,
                    files_found: 10,
                },
            ),
        ];

        let analysis = BreakEvenAnalysis::calculate(2000, &comparisons);

        // Schema: 2000 bytes = 500 tokens
        // Avg savings: ((400-100) + (600-200)) / 2 = 350 bytes = 87.5 tokens
        // Break-even: 500 / 87.5 ≈ 6 queries
        assert_eq!(analysis.schema_tokens, 500);
        assert!((analysis.avg_savings_bytes - 350.0).abs() < 0.01);
        assert!(analysis.break_even_queries <= 6);
    }

    #[test]
    fn test_benchmark_stats() {
        let samples = vec![10.0, 20.0, 30.0, 40.0, 50.0];
        let stats = BenchmarkStats::from_samples(&samples);

        assert!((stats.mean - 30.0).abs() < 0.01);
        assert!((stats.median - 30.0).abs() < 0.01);
        assert_eq!(stats.sample_count, 5);
        assert!(stats.cv_percent > 0.0);
    }

    #[test]
    fn test_cv_reliability() {
        let stats = BenchmarkStats {
            mean: 100.0,
            median: 100.0,
            std_dev: 10.0,
            cv_percent: 10.0,
            min: 90.0,
            max: 110.0,
            sample_count: 30,
        };

        assert!(stats.is_reliable(50.0));
        assert!(!stats.is_reliable(5.0));
    }
}
