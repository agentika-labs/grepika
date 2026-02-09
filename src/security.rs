//! Security module for grepika MCP server.
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
/// use grepika::security::validate_path;
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
    // Reject null bytes (defense-in-depth against C-level string truncation)
    if user_path.contains('\0') {
        return Err(SecurityError::PathTraversal {
            attempted: user_path.replace('\0', "\\0"),
            root: root.to_path_buf(),
        });
    }

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
        dunce::canonicalize(&joined).map_err(|_| SecurityError::PathTraversal {
            attempted: user_path.to_string(),
            root: root.to_path_buf(),
        })?
    } else {
        // For non-existent paths, verify the normalized path is safe
        let canonical_root = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        canonical_root.join(&normalized)
    };

    // Final verification: resolved path must start with root
    let canonical_root = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
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
            PatternMatchType::PathContains => {
                #[cfg(windows)]
                {
                    // Normalize backslashes for Windows compatibility
                    // (patterns use '/' but Windows paths use '\')
                    let normalized = path_str.replace('\\', "/");
                    normalized.contains(self.pattern)
                }
                #[cfg(not(windows))]
                {
                    path_str.contains(self.pattern)
                }
            }
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
/// use grepika::security::is_sensitive_file;
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
    // Reject null bytes (belt-and-suspenders with validate_path)
    if user_path.contains('\0') {
        return Err(SecurityError::PathTraversal {
            attempted: user_path.replace('\0', "\\0"),
            root: root.to_path_buf(),
        });
    }

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
// Workspace Root Validation
// ============================================================================

/// Returns true if the canonicalized path is a blocked system location.
///
/// On Unix: exact match against known system paths.
/// On Windows: structural analysis using `Path::components()` to handle
/// drive letter variation, case-insensitivity, and UNC paths.
fn is_blocked_system_path(canonical: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let components: Vec<_> = canonical.components().collect();

        // Block ALL drive roots (C:\, D:\, etc.)
        if matches!(
            components.as_slice(),
            [Component::Prefix(_), Component::RootDir] | [Component::Prefix(_)]
        ) {
            return true;
        }

        // Block UNC and device paths entirely (network shares).
        // Note: Prefix::VerbatimDisk is not listed because dunce::canonicalize()
        // strips the \\?\ prefix, converting VerbatimDisk to Disk before we get here.
        if let Some(Component::Prefix(p)) = components.first() {
            if matches!(
                p.kind(),
                Prefix::UNC(..)
                    | Prefix::VerbatimUNC(..)
                    | Prefix::DeviceNS(..)
                    | Prefix::Verbatim(..)
            ) {
                return true;
            }
        }

        // Block known system directories (case-insensitive)
        if components.len() == 3 {
            if let Some(Component::Normal(first_dir)) = components.get(2) {
                let dir_lower = first_dir.to_string_lossy().to_lowercase();
                let blocked = [
                    "windows",
                    "users",
                    "program files",
                    "program files (x86)",
                    "programdata",
                    "recovery",
                    "system volume information",
                ];
                if blocked.contains(&dir_lower.as_str()) {
                    return true;
                }
            }
        }
        false
    }
    #[cfg(not(windows))]
    {
        let canonical_str = canonical.to_string_lossy();
        const BLOCKED: &[&str] = &[
            "/",
            "/etc",
            "/var",
            "/usr",
            "/bin",
            "/sbin",
            "/sys",
            "/proc",
            "/dev",
            "/boot",
            "/tmp",
            "/lib",
            "/lib64",
            "/opt",
            // macOS symlink targets
            "/private/etc",
            "/private/var",
            "/private/tmp",
        ];
        BLOCKED.iter().any(|b| canonical_str == *b)
    }
}

/// Sensitive user directories that must never be used as workspace roots.
const BLOCKED_USER_DIRS: &[&str] = &[
    ".ssh", ".aws", ".gnupg", ".gpg", ".config", ".local/share",
];

/// Validates that a path is safe to use as a workspace root.
///
/// Security checks (applied to LLM-provided paths via `add_workspace`):
/// 1. Must exist and be a directory
/// 2. Must not be a system-critical path
/// 3. Must not be a sensitive user directory
/// 4. Must have at least 3 path components (blocks `/`, `/home`, `/Users`)
/// 5. Returns the canonicalized (symlink-resolved) path
///
/// The `--root` CLI flag bypasses this since it's user-provided, not LLM-provided.
pub fn validate_workspace_root(path: &Path) -> Result<PathBuf, String> {
    // Canonicalize first to resolve symlinks (e.g., /tmp/innocent -> /etc)
    let canonical = dunce::canonicalize(path).map_err(|e| {
        format!(
            "Cannot access '{}': {}. Ensure the path exists and is readable.",
            path.display(),
            e
        )
    })?;

    // Must be a directory
    if !canonical.is_dir() {
        return Err(format!(
            "'{}' is not a directory. Provide a project root directory path.",
            path.display()
        ));
    }

    // Check system-critical paths (structural on Windows, exact match on Unix)
    if is_blocked_system_path(&canonical) {
        return Err(format!(
            "Cannot use '{}' as workspace root: system-critical directory.",
            canonical.display()
        ));
    }

    // Check sensitive user directories
    let home_dir = dirs::home_dir();
    if let Some(ref home) = home_dir {
        // Reject home directory itself
        if canonical == *home {
            return Err(format!(
                "Cannot use home directory '{}' as workspace root. Use a project subdirectory.",
                canonical.display()
            ));
        }

        for sensitive in BLOCKED_USER_DIRS {
            let sensitive_path = home.join(sensitive);
            if let Ok(sensitive_canonical) = dunce::canonicalize(&sensitive_path) {
                if canonical.starts_with(&sensitive_canonical) {
                    return Err(format!(
                        "Cannot use '{}' as workspace root: contains sensitive user data.",
                        canonical.display()
                    ));
                }
            }
        }
    }

    // Minimum depth check: at least 3 components (e.g., /Users/adam/project)
    let component_count = canonical.components().count();
    if component_count < 3 {
        return Err(format!(
            "Path '{}' is too broad ({} components). Use a specific project directory.",
            canonical.display(),
            component_count
        ));
    }

    Ok(canonical)
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
/// use grepika::security::validate_regex_pattern;
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
    let mut chars = pattern.chars();

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

    #[test]
    fn test_null_byte_rejected() {
        let root = Path::new("/project");

        // Null byte in path should be rejected
        assert!(validate_path(root, "file.txt\0../../etc/passwd").is_err());
        assert!(validate_path(root, "\0").is_err());
        assert!(validate_path(root, "src/\0main.rs").is_err());

        // validate_read_access also rejects null bytes
        assert!(validate_read_access(root, "file.txt\0").is_err());
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

    // Workspace root validation tests (cross-platform)

    #[test]
    fn test_workspace_root_accepts_valid_project() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = validate_workspace_root(dir.path());
        // TempDir creates paths like /var/folders/xx/xx/T/xxx (deep enough)
        // or /tmp/xxx which resolves to /private/tmp/xxx on macOS (4 components)
        // On Windows: C:\Users\...\AppData\Local\Temp\xxx (deep enough)
        assert!(result.is_ok(), "Should accept valid temp dir: {:?}", result);
        // Result should be canonicalized
        let canonical = result.unwrap();
        assert!(canonical.is_absolute());
    }

    #[test]
    fn test_workspace_root_rejects_nonexistent() {
        #[cfg(unix)]
        let path = Path::new("/definitely/does/not/exist/xyzzy");
        #[cfg(windows)]
        let path = Path::new("C:\\definitely\\does\\not\\exist\\xyzzy");

        let err = validate_workspace_root(path);
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("Cannot access"));
    }

    #[test]
    fn test_workspace_root_rejects_home_directory() {
        if let Some(home) = dirs::home_dir() {
            let result = validate_workspace_root(&home);
            assert!(result.is_err(), "Should reject home directory");
        }
    }

    #[test]
    fn test_workspace_root_rejects_sensitive_user_dirs() {
        if let Some(home) = dirs::home_dir() {
            let ssh_dir = home.join(".ssh");
            if ssh_dir.exists() {
                let result = validate_workspace_root(&ssh_dir);
                assert!(result.is_err(), "Should reject ~/.ssh");
            }
        }
    }

    // Unix-specific workspace root tests

    #[cfg(unix)]
    #[test]
    fn test_workspace_root_rejects_system_paths() {
        assert!(validate_workspace_root(Path::new("/")).is_err());
        assert!(validate_workspace_root(Path::new("/etc")).is_err());
        assert!(validate_workspace_root(Path::new("/var")).is_err());
        assert!(validate_workspace_root(Path::new("/usr")).is_err());
        assert!(validate_workspace_root(Path::new("/bin")).is_err());
        assert!(validate_workspace_root(Path::new("/tmp")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_workspace_root_rejects_files() {
        // /etc/passwd exists on macOS/Linux and is a file, not a directory
        let result = validate_workspace_root(Path::new("/etc/passwd"));
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_workspace_root_rejects_too_shallow() {
        // Paths with < 3 components should be rejected
        let err = validate_workspace_root(Path::new("/"));
        assert!(err.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_workspace_root_resolves_symlinks() {
        let dir = tempfile::TempDir::new().unwrap();
        let real_dir = dir.path().join("real_project");
        std::fs::create_dir(&real_dir).unwrap();
        let link_path = dir.path().join("symlink_project");

        std::os::unix::fs::symlink(&real_dir, &link_path).unwrap();
        let result = validate_workspace_root(&link_path);
        assert!(result.is_ok());
        // Should resolve to the real path
        let canonical = result.unwrap();
        assert_eq!(
            canonical,
            dunce::canonicalize(&real_dir).unwrap(),
            "Should resolve symlink to real path"
        );
    }

    // Windows-specific workspace root tests

    #[cfg(windows)]
    #[test]
    fn test_workspace_root_rejects_drive_root() {
        // C:\ should be rejected as too broad / system-critical
        assert!(validate_workspace_root(Path::new("C:\\")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn test_workspace_root_rejects_system_dirs() {
        // Windows system directories (case-insensitive check)
        let windows_dir = Path::new("C:\\Windows");
        if windows_dir.exists() {
            assert!(
                validate_workspace_root(windows_dir).is_err(),
                "Should reject C:\\Windows"
            );
        }
        let program_files = Path::new("C:\\Program Files");
        if program_files.exists() {
            assert!(
                validate_workspace_root(program_files).is_err(),
                "Should reject C:\\Program Files"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn test_workspace_root_rejects_files_windows() {
        // A known file on Windows
        let result = validate_workspace_root(Path::new("C:\\Windows\\System32\\notepad.exe"));
        assert!(result.is_err());
    }

    #[cfg(windows)]
    #[test]
    fn test_workspace_root_resolves_symlinks_windows() {
        let dir = tempfile::TempDir::new().unwrap();
        let real_dir = dir.path().join("real_project");
        std::fs::create_dir(&real_dir).unwrap();
        let link_path = dir.path().join("symlink_project");

        match std::os::windows::fs::symlink_dir(&real_dir, &link_path) {
            Ok(()) => {
                let result = validate_workspace_root(&link_path);
                assert!(result.is_ok());
                let canonical = result.unwrap();
                assert_eq!(
                    canonical,
                    dunce::canonicalize(&real_dir).unwrap(),
                    "Should resolve symlink to real path"
                );
            }
            Err(e) if e.raw_os_error() == Some(1314) => {
                // ERROR_PRIVILEGE_NOT_HELD — symlink creation requires elevated privileges
                // Skip test gracefully on non-admin accounts
            }
            Err(e) => panic!("Unexpected error creating symlink: {}", e),
        }
    }

    // is_blocked_system_path unit tests

    #[cfg(unix)]
    #[test]
    fn test_is_blocked_system_path_unix() {
        assert!(is_blocked_system_path(Path::new("/")));
        assert!(is_blocked_system_path(Path::new("/etc")));
        assert!(is_blocked_system_path(Path::new("/var")));
        assert!(is_blocked_system_path(Path::new("/private/etc")));
        assert!(!is_blocked_system_path(Path::new("/home/user/project")));
        assert!(!is_blocked_system_path(Path::new("/Users/adam/code")));
    }

    #[cfg(windows)]
    #[test]
    fn test_is_blocked_system_path_windows() {
        use std::path::PathBuf;

        // Drive roots
        assert!(is_blocked_system_path(&PathBuf::from("C:\\")));
        assert!(is_blocked_system_path(&PathBuf::from("D:\\")));

        // System directories
        assert!(is_blocked_system_path(&PathBuf::from("C:\\Windows")));
        assert!(is_blocked_system_path(&PathBuf::from("C:\\Program Files")));
        assert!(is_blocked_system_path(&PathBuf::from("C:\\Program Files (x86)")));
        assert!(is_blocked_system_path(&PathBuf::from("C:\\ProgramData")));

        // C:\Users itself should be blocked (contains all user home dirs)
        assert!(is_blocked_system_path(&PathBuf::from("C:\\Users")));

        // Valid project paths should not be blocked
        assert!(!is_blocked_system_path(&PathBuf::from(
            "C:\\Users\\adam\\project"
        )));
        assert!(!is_blocked_system_path(&PathBuf::from(
            "D:\\code\\my-app"
        )));

        // Subdirectories of system dirs should not be blocked
        // (only the top-level system dir itself is blocked)
        assert!(!is_blocked_system_path(&PathBuf::from(
            "C:\\Windows\\Temp\\my-project"
        )));
    }
}
