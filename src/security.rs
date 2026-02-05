//! Security module for agentika-grep MCP server.
//!
//! Provides protection against:
//! - Path traversal attacks (escaping root directory)
//! - Sensitive file exposure (.env, credentials, keys)
//! - ReDoS attacks (regex complexity limits)
//!
//! # Design Philosophy
//!
//! Performance exclusions (node_modules, dist, lock files) remain controlled
//! by `.gitignore`, allowing users to search dependencies when debugging.
//! Only **security-sensitive files** are hardcoded exclusions.

use std::path::{Component, Path, PathBuf};
use thiserror::Error;

/// Security-related errors.
#[derive(Error, Debug, Clone)]
pub enum SecurityError {
    #[error("Path traversal blocked: '{attempted}' escapes root '{}'", root.display())]
    PathTraversal { attempted: String, root: PathBuf },

    #[error("Access denied: '{path}' is a sensitive file ({reason})")]
    SensitiveFile { path: String, reason: &'static str },

    #[error("Regex pattern rejected: {reason}")]
    DangerousPattern { pattern: String, reason: &'static str },

    #[error("Absolute path not allowed: '{path}'")]
    AbsolutePath { path: String },
}

impl SecurityError {
    /// Returns a machine-readable error code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::PathTraversal { .. } => "PATH_TRAVERSAL",
            Self::SensitiveFile { .. } => "SENSITIVE_FILE",
            Self::DangerousPattern { .. } => "DANGEROUS_PATTERN",
            Self::AbsolutePath { .. } => "ABSOLUTE_PATH",
        }
    }
}

// ============================================================================
// Path Validation
// ============================================================================

/// Validates that a user-provided path stays within the root directory.
///
/// # Security Properties
///
/// 1. Rejects absolute paths
/// 2. Normalizes path components (resolves `.` and `..`)
/// 3. Ensures final path starts with root
///
/// # Example
///
/// ```
/// use agentika_grep::security::validate_path;
/// use std::path::Path;
///
/// let root = Path::new("/project");
///
/// // Valid paths
/// assert!(validate_path(root, "src/main.rs").is_ok());
/// assert!(validate_path(root, "./src/../lib.rs").is_ok());
///
/// // Invalid paths (traversal attempts)
/// assert!(validate_path(root, "../etc/passwd").is_err());
/// assert!(validate_path(root, "/etc/passwd").is_err());
/// assert!(validate_path(root, "src/../../etc/passwd").is_err());
/// ```
pub fn validate_path(root: &Path, user_path: &str) -> Result<PathBuf, SecurityError> {
    let user_path_obj = Path::new(user_path);

    // Reject absolute paths immediately
    if user_path_obj.is_absolute() {
        return Err(SecurityError::AbsolutePath {
            path: user_path.to_string(),
        });
    }

    // Normalize the path without filesystem access
    let normalized = normalize_path(user_path_obj);

    // Check for any remaining parent directory components
    for component in normalized.components() {
        if matches!(component, Component::ParentDir) {
            return Err(SecurityError::PathTraversal {
                attempted: user_path.to_string(),
                root: root.to_path_buf(),
            });
        }
    }

    // Join with root and verify it stays within root
    let joined = root.join(&normalized);

    // Try to canonicalize for existing paths, use normalized for non-existent
    let resolved = if joined.exists() {
        joined.canonicalize().map_err(|_| SecurityError::PathTraversal {
            attempted: user_path.to_string(),
            root: root.to_path_buf(),
        })?
    } else {
        // For non-existent paths, verify the normalized path is safe
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        canonical_root.join(&normalized)
    };

    // Final verification: resolved path must start with root
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if !resolved.starts_with(&canonical_root) {
        return Err(SecurityError::PathTraversal {
            attempted: user_path.to_string(),
            root: root.to_path_buf(),
        });
    }

    Ok(resolved)
}

/// Normalizes a path by resolving `.` and `..` components without filesystem access.
///
/// This is a pure function that operates on path components only.
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();

    for component in path.components() {
        match component {
            Component::CurDir => {
                // Skip "." components
            }
            Component::ParentDir => {
                // Pop the last component if possible, otherwise keep ".."
                if components.last().is_some_and(|c| !matches!(c, Component::ParentDir)) {
                    components.pop();
                } else {
                    components.push(component);
                }
            }
            _ => {
                components.push(component);
            }
        }
    }

    components.iter().collect()
}

// ============================================================================
// Sensitive File Detection
// ============================================================================

/// Comprehensive list of sensitive file patterns.
///
/// These patterns cover credentials, secrets, keys, and other security-sensitive
/// files that should never be exposed through the MCP server, regardless of
/// gitignore settings.
pub const SENSITIVE_PATTERNS: &[SensitivePattern] = &[
    // === Environment & Secrets ===
    SensitivePattern::exact(".env", "environment variables"),
    SensitivePattern::prefix(".env.", "environment variables"),
    SensitivePattern::suffix(".env", "environment variables"),
    SensitivePattern::exact(".envrc", "direnv config"),

    // === Credentials & Config ===
    SensitivePattern::exact("credentials.json", "credentials file"),
    SensitivePattern::exact("credentials.yaml", "credentials file"),
    SensitivePattern::exact("credentials.yml", "credentials file"),
    SensitivePattern::exact("secrets.json", "secrets file"),
    SensitivePattern::exact("secrets.yaml", "secrets file"),
    SensitivePattern::exact("secrets.yml", "secrets file"),
    SensitivePattern::contains(".secret.", "secrets file"),

    // === Private Keys & Certificates ===
    SensitivePattern::suffix(".pem", "private key/certificate"),
    SensitivePattern::suffix(".key", "private key"),
    SensitivePattern::suffix(".p12", "PKCS#12 keystore"),
    SensitivePattern::suffix(".pfx", "PKCS#12 keystore"),
    SensitivePattern::suffix(".jks", "Java keystore"),
    SensitivePattern::suffix(".keystore", "keystore"),
    SensitivePattern::suffix(".truststore", "truststore"),
    SensitivePattern::exact("id_rsa", "SSH private key"),
    SensitivePattern::prefix("id_rsa.", "SSH private key"),
    SensitivePattern::exact("id_ed25519", "SSH private key"),
    SensitivePattern::prefix("id_ed25519.", "SSH private key"),
    SensitivePattern::exact("id_ecdsa", "SSH private key"),
    SensitivePattern::prefix("id_ecdsa.", "SSH private key"),
    SensitivePattern::exact("id_dsa", "SSH private key"),
    SensitivePattern::prefix("id_dsa.", "SSH private key"),
    SensitivePattern::suffix(".kdbx", "KeePass database"),

    // === Cloud Provider Credentials ===
    SensitivePattern::path_contains(".aws/credentials", "AWS credentials"),
    SensitivePattern::path_contains(".aws/config", "AWS config"),
    SensitivePattern::path_contains(".azure/credentials", "Azure credentials"),
    SensitivePattern::exact("gcloud-credentials.json", "GCloud credentials"),
    SensitivePattern::prefix("service-account", "service account key"),
    SensitivePattern::exact("application_default_credentials.json", "GCloud ADC"),
    SensitivePattern::path_contains(".digitalocean/config.yaml", "DigitalOcean config"),

    // === Infrastructure as Code ===
    SensitivePattern::exact("terraform.tfstate", "Terraform state"),
    SensitivePattern::prefix("terraform.tfstate.", "Terraform state backup"),
    SensitivePattern::suffix(".tfstate", "Terraform state"),
    SensitivePattern::suffix(".tfvars", "Terraform variables (may contain secrets)"),
    SensitivePattern::exact("kubeconfig", "Kubernetes config"),
    SensitivePattern::suffix(".kubeconfig", "Kubernetes config"),
    SensitivePattern::path_contains(".kube/config", "Kubernetes config"),
    SensitivePattern::exact(".vault-token", "Vault token"),

    // === Package Manager Auth ===
    SensitivePattern::exact(".npmrc", "npm config (may contain tokens)"),
    SensitivePattern::exact(".yarnrc", "yarn config"),
    SensitivePattern::exact(".yarnrc.yml", "yarn config"),
    SensitivePattern::exact(".pypirc", "PyPI credentials"),
    SensitivePattern::exact("pip.conf", "pip config"),
    SensitivePattern::path_contains(".gem/credentials", "RubyGems credentials"),
    SensitivePattern::path_contains(".bundle/config", "Bundler config"),
    SensitivePattern::path_contains(".docker/config.json", "Docker credentials"),
    SensitivePattern::path_contains(".nuget/NuGet.Config", "NuGet config"),

    // === Git & CI/CD ===
    SensitivePattern::exact(".git-credentials", "Git credentials"),
    SensitivePattern::exact(".netrc", "network credentials"),
    SensitivePattern::exact("_netrc", "network credentials (Windows)"),

    // === Application Secrets ===
    SensitivePattern::exact("master.key", "Rails master key"),
    SensitivePattern::suffix(".master.key", "master key"),
    SensitivePattern::exact("encryption.key", "encryption key"),
    SensitivePattern::exact("signing.key", "signing key"),

    // === Web Server & Auth ===
    SensitivePattern::exact(".htpasswd", "htpasswd file"),
    SensitivePattern::suffix(".htpasswd", "htpasswd file"),
    SensitivePattern::exact("shadow", "shadow password file"),

    // === SSH Config ===
    SensitivePattern::path_contains(".ssh/config", "SSH config"),
    SensitivePattern::exact("authorized_keys", "SSH authorized keys"),

    // === IDE Secrets ===
    SensitivePattern::path_contains(".idea/dataSources.xml", "JetBrains DB connections"),
    SensitivePattern::path_contains(".idea/webServers.xml", "JetBrains server creds"),

    // === History files (may contain secrets) ===
    SensitivePattern::exact(".bash_history", "shell history"),
    SensitivePattern::exact(".zsh_history", "shell history"),
    SensitivePattern::exact(".psql_history", "psql history"),
    SensitivePattern::exact(".mysql_history", "mysql history"),
    SensitivePattern::exact(".node_repl_history", "Node REPL history"),
];

/// A pattern for matching sensitive files.
#[derive(Debug, Clone, Copy)]
pub struct SensitivePattern {
    /// The pattern string to match
    pub pattern: &'static str,
    /// Description of why this is sensitive
    pub reason: &'static str,
    /// How to match the pattern
    pub match_type: PatternMatchType,
}

/// How to match a sensitive pattern.
#[derive(Debug, Clone, Copy)]
pub enum PatternMatchType {
    /// Exact filename match
    Exact,
    /// Filename starts with pattern
    Prefix,
    /// Filename ends with pattern
    Suffix,
    /// Filename contains pattern
    Contains,
    /// Full path contains pattern
    PathContains,
}

impl SensitivePattern {
    const fn exact(pattern: &'static str, reason: &'static str) -> Self {
        Self {
            pattern,
            reason,
            match_type: PatternMatchType::Exact,
        }
    }

    const fn prefix(pattern: &'static str, reason: &'static str) -> Self {
        Self {
            pattern,
            reason,
            match_type: PatternMatchType::Prefix,
        }
    }

    const fn suffix(pattern: &'static str, reason: &'static str) -> Self {
        Self {
            pattern,
            reason,
            match_type: PatternMatchType::Suffix,
        }
    }

    const fn contains(pattern: &'static str, reason: &'static str) -> Self {
        Self {
            pattern,
            reason,
            match_type: PatternMatchType::Contains,
        }
    }

    const fn path_contains(pattern: &'static str, reason: &'static str) -> Self {
        Self {
            pattern,
            reason,
            match_type: PatternMatchType::PathContains,
        }
    }

    /// Checks if the given path matches this sensitive pattern.
    #[must_use]
    pub fn matches(&self, path: &Path) -> bool {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let path_str = path.to_string_lossy();

        match self.match_type {
            PatternMatchType::Exact => filename == self.pattern,
            PatternMatchType::Prefix => filename.starts_with(self.pattern),
            PatternMatchType::Suffix => filename.ends_with(self.pattern),
            PatternMatchType::Contains => filename.contains(self.pattern),
            PatternMatchType::PathContains => path_str.contains(self.pattern),
        }
    }
}

/// Checks if a path points to a sensitive file.
///
/// Returns `Some(reason)` if the file is sensitive, `None` otherwise.
///
/// # Example
///
/// ```
/// use agentika_grep::security::is_sensitive_file;
/// use std::path::Path;
///
/// assert!(is_sensitive_file(Path::new(".env")).is_some());
/// assert!(is_sensitive_file(Path::new(".env.local")).is_some());
/// assert!(is_sensitive_file(Path::new("id_rsa")).is_some());
/// assert!(is_sensitive_file(Path::new("main.rs")).is_none());
/// ```
#[must_use]
pub fn is_sensitive_file(path: &Path) -> Option<&'static str> {
    for pattern in SENSITIVE_PATTERNS {
        if pattern.matches(path) {
            return Some(pattern.reason);
        }
    }
    None
}

/// Validates a path is safe to read (not traversal, not sensitive).
///
/// This combines path traversal validation and sensitive file checking.
pub fn validate_read_access(root: &Path, user_path: &str) -> Result<PathBuf, SecurityError> {
    // First validate path traversal
    let resolved = validate_path(root, user_path)?;

    // Then check for sensitive files
    if let Some(reason) = is_sensitive_file(&resolved) {
        return Err(SecurityError::SensitiveFile {
            path: user_path.to_string(),
            reason,
        });
    }

    // Also check the user-provided path (in case the resolved path differs)
    if let Some(reason) = is_sensitive_file(Path::new(user_path)) {
        return Err(SecurityError::SensitiveFile {
            path: user_path.to_string(),
            reason,
        });
    }

    Ok(resolved)
}

// ============================================================================
// ReDoS Protection
// ============================================================================

/// Maximum allowed pattern length.
pub const MAX_PATTERN_LENGTH: usize = 500;

/// Maximum nesting depth for groups.
pub const MAX_NESTING_DEPTH: usize = 5;

/// Validates a regex pattern for potential ReDoS vulnerabilities.
///
/// Checks for:
/// 1. Pattern length limits
/// 2. Excessive nesting depth
/// 3. Known dangerous patterns (e.g., `(a+)+`, `(.*)*`)
///
/// # Example
///
/// ```
/// use agentika_grep::security::validate_regex_pattern;
///
/// // Safe patterns
/// assert!(validate_regex_pattern("fn\\s+\\w+").is_ok());
/// assert!(validate_regex_pattern("hello.*world").is_ok());
///
/// // Dangerous patterns
/// assert!(validate_regex_pattern("(a+)+$").is_err());
/// assert!(validate_regex_pattern("(.*)*").is_err());
/// ```
pub fn validate_regex_pattern(pattern: &str) -> Result<(), SecurityError> {
    // Length check
    if pattern.len() > MAX_PATTERN_LENGTH {
        return Err(SecurityError::DangerousPattern {
            pattern: pattern.chars().take(50).collect::<String>() + "...",
            reason: "pattern exceeds maximum length",
        });
    }

    // Nesting depth check
    let nesting = count_nesting_depth(pattern);
    if nesting > MAX_NESTING_DEPTH {
        return Err(SecurityError::DangerousPattern {
            pattern: pattern.to_string(),
            reason: "excessive nesting depth",
        });
    }

    // Check for dangerous patterns that cause exponential backtracking
    if has_dangerous_quantifier_nesting(pattern) {
        return Err(SecurityError::DangerousPattern {
            pattern: pattern.to_string(),
            reason: "nested quantifiers can cause exponential backtracking",
        });
    }

    Ok(())
}

/// Counts the maximum nesting depth of groups in a pattern.
fn count_nesting_depth(pattern: &str) -> usize {
    let mut max_depth: usize = 0;
    let mut current_depth: usize = 0;
    let mut chars = pattern.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // Skip escaped character
                chars.next();
            }
            '(' => {
                current_depth += 1;
                max_depth = max_depth.max(current_depth);
            }
            ')' => {
                current_depth = current_depth.saturating_sub(1);
            }
            '[' => {
                // Skip character class
                while let Some(c) = chars.next() {
                    if c == '\\' {
                        chars.next();
                    } else if c == ']' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    max_depth
}

/// Detects dangerous patterns with nested quantifiers.
///
/// Patterns like `(a+)+`, `(.*)*`, `(a*)*` can cause exponential
/// backtracking (ReDoS).
fn has_dangerous_quantifier_nesting(pattern: &str) -> bool {
    // Common dangerous patterns
    let dangerous_patterns = [
        // Nested plus/star quantifiers
        r"(\w+)+",
        r"(.*)+",
        r"(.+)+",
        r"(\d+)+",
        r"(\s+)+",
        r"([^x]+)+",
        r"(\w*)*",
        r"(.*)*",
        r"(.+)*",
        r"(\d*)*",
        r"(\s*)*",
        // Alternation with overlapping patterns
        r"(a|a)+",
        r"(a|aa)+",
        r"(.*|.*)+",
    ];

    let pattern_lower = pattern.to_lowercase();

    for dangerous in dangerous_patterns {
        // Simple substring check - not perfect but catches common cases
        let dangerous_normalized = dangerous.to_lowercase();
        if pattern_lower.contains(&dangerous_normalized) {
            return true;
        }
    }

    // Heuristic: look for quantifier immediately after a group that contains a quantifier
    // This catches patterns like (x+)+ or (x*)+
    let re = regex::Regex::new(r"\([^)]*[+*][^)]*\)[+*?]").ok();
    if let Some(re) = re {
        if re.is_match(pattern) {
            return true;
        }
    }

    false
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Path validation tests

    #[test]
    fn test_valid_paths() {
        let root = Path::new("/project");

        assert!(validate_path(root, "src/main.rs").is_ok());
        assert!(validate_path(root, "lib.rs").is_ok());
        assert!(validate_path(root, "./src/lib.rs").is_ok());
        assert!(validate_path(root, "src/./lib.rs").is_ok());
    }

    #[test]
    fn test_path_traversal_blocked() {
        let root = Path::new("/project");

        // Direct traversal
        assert!(validate_path(root, "../etc/passwd").is_err());
        assert!(validate_path(root, "../../etc/passwd").is_err());

        // Hidden traversal
        assert!(validate_path(root, "src/../../etc/passwd").is_err());
        assert!(validate_path(root, "foo/../../../etc/passwd").is_err());

        // Absolute paths
        assert!(validate_path(root, "/etc/passwd").is_err());
        assert!(validate_path(root, "/root/.ssh/id_rsa").is_err());
    }

    #[test]
    fn test_normalize_path() {
        assert_eq!(normalize_path(Path::new("./foo")), Path::new("foo"));
        assert_eq!(normalize_path(Path::new("foo/./bar")), Path::new("foo/bar"));
        assert_eq!(normalize_path(Path::new("foo/../bar")), Path::new("bar"));
        assert_eq!(
            normalize_path(Path::new("foo/bar/../baz")),
            Path::new("foo/baz")
        );
        // Parent escaping root results in just ".."
        assert_eq!(normalize_path(Path::new("../foo")), Path::new("../foo"));
    }

    // Sensitive file detection tests

    #[test]
    fn test_sensitive_env_files() {
        assert!(is_sensitive_file(Path::new(".env")).is_some());
        assert!(is_sensitive_file(Path::new(".env.local")).is_some());
        assert!(is_sensitive_file(Path::new(".env.production")).is_some());
        assert!(is_sensitive_file(Path::new("production.env")).is_some());
        assert!(is_sensitive_file(Path::new(".envrc")).is_some());
    }

    #[test]
    fn test_sensitive_cloud_credentials() {
        assert!(is_sensitive_file(Path::new(".aws/credentials")).is_some());
        assert!(is_sensitive_file(Path::new("home/.aws/credentials")).is_some());
        assert!(is_sensitive_file(Path::new("service-account-key.json")).is_some());
        assert!(is_sensitive_file(Path::new("kubeconfig")).is_some());
        assert!(is_sensitive_file(Path::new("terraform.tfstate")).is_some());
        assert!(is_sensitive_file(Path::new("prod.tfvars")).is_some());
    }

    #[test]
    fn test_sensitive_keys() {
        assert!(is_sensitive_file(Path::new("id_rsa")).is_some());
        assert!(is_sensitive_file(Path::new("id_rsa.pub")).is_some());
        assert!(is_sensitive_file(Path::new("id_ed25519")).is_some());
        assert!(is_sensitive_file(Path::new("server.key")).is_some());
        assert!(is_sensitive_file(Path::new("cert.pem")).is_some());
        assert!(is_sensitive_file(Path::new("keystore.jks")).is_some());
    }

    #[test]
    fn test_non_sensitive_files() {
        assert!(is_sensitive_file(Path::new("main.rs")).is_none());
        assert!(is_sensitive_file(Path::new("config.toml")).is_none());
        assert!(is_sensitive_file(Path::new("package.json")).is_none());
        assert!(is_sensitive_file(Path::new("README.md")).is_none());
        assert!(is_sensitive_file(Path::new("src/lib.rs")).is_none());
    }

    // ReDoS protection tests

    #[test]
    fn test_safe_patterns() {
        assert!(validate_regex_pattern("fn\\s+\\w+").is_ok());
        assert!(validate_regex_pattern("hello.*world").is_ok());
        assert!(validate_regex_pattern("[a-z]+").is_ok());
        assert!(validate_regex_pattern("\\bfunction\\b").is_ok());
    }

    #[test]
    fn test_dangerous_patterns() {
        // Nested quantifiers
        assert!(validate_regex_pattern("(a+)+").is_err());
        assert!(validate_regex_pattern("(.*)*").is_err());
        assert!(validate_regex_pattern("(.+)+").is_err());
        assert!(validate_regex_pattern("(\\w+)+").is_err());
    }

    #[test]
    fn test_pattern_length_limit() {
        let long_pattern = "a".repeat(MAX_PATTERN_LENGTH + 1);
        assert!(validate_regex_pattern(&long_pattern).is_err());

        let ok_pattern = "a".repeat(MAX_PATTERN_LENGTH);
        assert!(validate_regex_pattern(&ok_pattern).is_ok());
    }

    #[test]
    fn test_nesting_depth() {
        // Within limit
        assert!(validate_regex_pattern("((((a))))").is_ok());

        // Exceeds limit (6 levels)
        assert!(validate_regex_pattern("((((((a))))))").is_err());
    }

    #[test]
    fn test_count_nesting() {
        assert_eq!(count_nesting_depth("abc"), 0);
        assert_eq!(count_nesting_depth("(abc)"), 1);
        assert_eq!(count_nesting_depth("((abc))"), 2);
        assert_eq!(count_nesting_depth("(a(b)c)"), 2);
        assert_eq!(count_nesting_depth("(a)(b)"), 1);
        assert_eq!(count_nesting_depth("\\(abc\\)"), 0); // Escaped
    }

    // Combined validation tests

    #[test]
    fn test_validate_read_access() {
        let root = Path::new("/project");

        // Traversal blocked
        assert!(matches!(
            validate_read_access(root, "../etc/passwd"),
            Err(SecurityError::PathTraversal { .. })
        ));

        // Sensitive file blocked
        assert!(matches!(
            validate_read_access(root, ".env"),
            Err(SecurityError::SensitiveFile { .. })
        ));
    }
}
